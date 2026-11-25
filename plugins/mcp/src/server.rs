//! MCP "Streamable HTTP" on 127.0.0.1 with std only: one POST endpoint (`/mcp`) that answers JSON-RPC with `application/json`.
//! No server-sent events: GET answers 405, which the spec allows. Every request must come from a loopback peer, name a
//! loopback `Host` (DNS rebinding), carry no foreign `Origin` (web pages) and the bearer token. A connection serves one request
//! and is closed.
//!
//! Threads: one accept thread (blocking `accept`, no polling; `stop` wakes it with a connection of its own) and one short-lived
//! thread per connection (at most `MAX_CONN`). `Server::stop` ends the accept thread and hands back the connection threads, so
//! the plugin can join them before its own `stop` returns.
use crate::mcp::{self, Api, Policy};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

const MAX_CONN: usize = 16;
const MAX_HEADER: usize = 16 * 1024;
const MAX_BODY: usize = 1024 * 1024;
const MAX_SESSIONS: usize = 64;
/// A slow or stalled client is dropped after this long.
const IO_TIMEOUT: Duration = Duration::from_secs(15);

/// What every connection reads: the current policy and token (changed by the settings) and the sessions.
#[derive(Default)]
pub struct State {
    pub policy: Policy, pub token: String,
    /// (session id, client name from `initialize`), oldest first.
    pub sessions: VecDeque<(String, String)>,
}
pub type Shared = Arc<Mutex<State>>;
pub fn lock(s: &Mutex<State>) -> std::sync::MutexGuard<'_, State> { s.lock().unwrap_or_else(|e| e.into_inner()) }

pub struct Req { pub method: String, pub path: String, pub headers: Vec<(String, String)>, pub body: Vec<u8> }
impl Req {
    pub fn header(&self, n: &str) -> Option<&str> { self.headers.iter().find(|h| h.0.eq_ignore_ascii_case(n)).map(|h| h.1.as_str()) }
}
#[derive(Debug, PartialEq)]
pub struct Resp { pub status: u16, pub headers: Vec<(String, String)>, pub body: String }
fn resp(status: u16, body: &str) -> Resp {
    let ct = if body.is_empty() { vec![] } else if body.starts_with(['{', '[']) { vec![("Content-Type".into(), "application/json".into())] } else { vec![("Content-Type".into(), "text/plain; charset=utf-8".into())] };
    Resp { status, headers: ct, body: body.into() }
}

/// `Host: 127.0.0.1:<port>` (or `localhost`, `[::1]`): a page that rebinds its own DNS name to 127.0.0.1 still sends its name.
pub fn host_ok(h: Option<&str>, port: u16) -> bool {
    h.is_some_and(|h| ["127.0.0.1", "localhost", "[::1]"].iter().any(|n| h.eq_ignore_ascii_case(&format!("{n}:{port}"))))
}
/// No `Origin` (not a browser), or a page on this machine (`http(s)://localhost|127.0.0.1|[::1][:port]`).
pub fn origin_ok(o: Option<&str>) -> bool {
    let Some(o) = o else { return true };
    let Some(rest) = o.strip_prefix("http://").or_else(|| o.strip_prefix("https://")) else { return false };
    let host = if rest.starts_with('[') { rest.split_inclusive(']').next().unwrap_or("") } else { rest.split(':').next().unwrap_or("") };
    let port_ok = rest[host.len()..].is_empty() || rest[host.len()..].strip_prefix(':').is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    ["localhost", "127.0.0.1", "[::1]"].iter().any(|n| host.eq_ignore_ascii_case(n)) && port_ok
}
/// `Authorization: Bearer <token>`, compared in constant time. An empty token never matches.
pub fn auth_ok(h: Option<&str>, token: &str) -> bool {
    let Some(given) = h.and_then(|h| h.split_once(' ')).filter(|(s, _)| s.eq_ignore_ascii_case("bearer")).map(|(_, t)| t.trim()) else { return false };
    !token.is_empty() && given.len() == token.len() && given.bytes().zip(token.bytes()).fold(0u8, |d, (a, b)| d | (a ^ b)) == 0
}

/// Answers one request (everything but the socket). `new_id` makes a session id for `initialize`.
pub fn route(api: &dyn Api, st: &Shared, port: u16, req: &Req, new_id: &mut dyn FnMut() -> String) -> Resp {
    if !host_ok(req.header("host"), port) || !origin_ok(req.header("origin")) {
        api.log("rejected: foreign host or origin");
        return resp(403, "Forbidden");
    }
    if req.path.split('?').next() != Some("/mcp") { return resp(404, "Not Found"); }
    let (policy, token) = { let s = lock(st); (s.policy.clone(), s.token.clone()) };
    if !auth_ok(req.header("authorization"), &token) {
        api.log("rejected: request without a valid token");
        let mut r = resp(401, "Unauthorized");
        r.headers.push(("WWW-Authenticate".into(), "Bearer".into()));
        return r;
    }
    let session = req.header("mcp-session-id").map(str::to_string);
    match req.method.as_str() {
        "POST" => {}
        "DELETE" => {
            if let Some(sid) = &session { lock(st).sessions.retain(|s| &s.0 != sid); }
            return resp(200, "");
        }
        _ => { let mut r = resp(405, "Method Not Allowed"); r.headers.push(("Allow".into(), "POST, DELETE".into())); return r; }
    }
    if let Some(v) = req.header("mcp-protocol-version") { if !mcp::VERSIONS.contains(&v) { return resp(400, "Unsupported MCP-Protocol-Version"); } }
    if !req.header("content-type").is_some_and(|c| c.to_ascii_lowercase().starts_with("application/json")) { return resp(415, "Unsupported Media Type"); }
    let Ok(body) = serde_json::from_slice::<Value>(&req.body) else {
        return resp(400, &json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "Parse error" } }).to_string());
    };
    // The client's name for logs and the confirmation dialog: from `initialize`, else from its session.
    let msgs: Vec<&Value> = match &body { Value::Array(a) => a.iter().collect(), v => vec![v] };
    if msgs.is_empty() { return resp(400, &json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32600, "message": "Invalid Request" } }).to_string()); }
    let init = msgs.iter().find(|m| m["method"] == "initialize");
    let known = session.as_ref().and_then(|sid| lock(st).sessions.iter().find(|s| &s.0 == sid).map(|s| s.1.clone()));
    if session.is_some() && known.is_none() && init.is_none() { return resp(404, "Unknown session"); }
    let client = match init {
        Some(m) => clean_name(m["params"]["clientInfo"]["name"].as_str().unwrap_or("")),
        None => known.unwrap_or_else(|| "MCP client".into()),
    };
    let answers: Vec<Value> = msgs.iter().filter_map(|m| mcp::handle(api, &policy, &client, m)).collect();
    let mut r = if answers.is_empty() { resp(202, "") } else if body.is_array() { resp(200, &Value::Array(answers).to_string()) } else { resp(200, &answers[0].to_string()) };
    if init.is_some() {
        let sid = new_id();
        let mut s = lock(st);
        s.sessions.push_back((sid.clone(), client));
        while s.sessions.len() > MAX_SESSIONS { s.sessions.pop_front(); }
        r.headers.push(("Mcp-Session-Id".into(), sid));
    }
    r
}

/// A client name as shown in logs and dialogs: printable, at most 60 characters.
pub fn clean_name(n: &str) -> String {
    let n: String = n.chars().filter(|c| !c.is_control()).take(60).collect();
    if n.trim().is_empty() { "MCP client".into() } else { n.trim().to_string() }
}

/// Reads one HTTP/1.1 request (Content-Length bodies only); the status to answer when it is unusable.
pub fn read_req(s: impl Read) -> Result<Req, u16> {
    let mut r = BufReader::new(s.take((MAX_HEADER + MAX_BODY) as u64));
    let mut line = String::new();
    let mut head = 0;
    let mut next = |r: &mut BufReader<_>, line: &mut String| -> Result<(), u16> {
        line.clear();
        let n = r.read_line(line).map_err(|_| 400u16)?;
        head += n;
        if n == 0 || head > MAX_HEADER { return Err(if n == 0 { 400 } else { 431 }); }
        Ok(())
    };
    next(&mut r, &mut line)?;
    let mut parts = line.split_whitespace();
    let (Some(method), Some(path)) = (parts.next().map(str::to_string), parts.next().map(str::to_string)) else { return Err(400) };
    let mut headers = vec![];
    loop {
        next(&mut r, &mut line)?;
        let l = line.trim_end_matches(['\r', '\n']);
        if l.is_empty() { break; }
        let (k, v) = l.split_once(':').ok_or(400u16)?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let req = Req { method, path, headers, body: vec![] };
    if req.header("transfer-encoding").is_some() { return Err(411); }
    let len: usize = req.header("content-length").map_or(Ok(0), |v| v.parse().map_err(|_| 400u16))?;
    if len > MAX_BODY { return Err(413); }
    let mut body = vec![0; len];
    r.read_exact(&mut body).map_err(|_| 400u16)?;
    Ok(Req { body, ..req })
}

fn reason(s: u16) -> &'static str {
    match s { 200 => "OK", 202 => "Accepted", 400 => "Bad Request", 401 => "Unauthorized", 403 => "Forbidden", 404 => "Not Found", 405 => "Method Not Allowed",
        411 => "Length Required", 413 => "Payload Too Large", 415 => "Unsupported Media Type", 431 => "Request Header Fields Too Large", 503 => "Service Unavailable", _ => "" }
}
pub fn write_resp(mut w: impl Write, r: &Resp) -> std::io::Result<()> {
    let mut out = format!("HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n", r.status, reason(r.status), r.body.len());
    for (k, v) in &r.headers { out.push_str(&format!("{k}: {v}\r\n")); }
    out.push_str("\r\n");
    out.push_str(&r.body);
    w.write_all(out.as_bytes())?;
    w.flush()
}

pub struct Server { pub port: u16, stop: Arc<AtomicBool>, accept: Option<JoinHandle<()>>, conns: Arc<Mutex<Vec<JoinHandle<()>>>> }

/// Listens on 127.0.0.1:`port` (0 = any free port, for tests).
pub fn start(port: u16, api: Arc<dyn Api>, st: Shared, new_id: fn() -> String) -> std::io::Result<Server> {
    let l = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port))?;
    let port = l.local_addr()?.port();
    let stop = Arc::new(AtomicBool::new(false));
    let conns: Arc<Mutex<Vec<JoinHandle<()>>>> = Default::default();
    let (stop2, conns2) = (stop.clone(), conns.clone());
    let accept = std::thread::spawn(move || {
        for s in l.incoming() {
            if stop2.load(Ordering::SeqCst) { break; }
            let Ok(s) = s else { continue };
            let mut c = conns2.lock().unwrap_or_else(|e| e.into_inner());
            c.retain(|h| !h.is_finished());
            if c.len() >= MAX_CONN { let _ = write_resp(&s, &resp(503, "Busy")); continue; }
            let (api, st) = (api.clone(), st.clone());
            c.push(std::thread::spawn(move || serve(s, &*api, &st, port, new_id)));
        }
    });
    Ok(Server { port, stop, accept: Some(accept), conns })
}

/// Only loopback peers are served (the socket is bound to 127.0.0.1, so this is a second fence).
pub fn peer_ok(ip: IpAddr) -> bool { ip.is_loopback() || matches!(ip, IpAddr::V6(v6) if v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())) }

fn serve(s: TcpStream, api: &dyn Api, st: &Shared, port: u16, new_id: fn() -> String) {
    if !s.peer_addr().is_ok_and(|p| peer_ok(p.ip())) { return; }
    let _ = s.set_read_timeout(Some(IO_TIMEOUT));
    let _ = s.set_write_timeout(Some(IO_TIMEOUT));
    let r = match read_req(&s) { Ok(req) => route(api, st, port, &req, &mut || new_id()), Err(code) => resp(code, reason(code)) };
    let _ = write_resp(&s, &r);
}

impl Server {
    /// Stops accepting and ends the accept thread. The connection threads still running are handed back to be joined (one may
    /// wait for a confirmation in Webox, which answers "no" once the plugin stops).
    pub fn stop(mut self) -> Vec<JoinHandle<()>> {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the blocking accept.
        let _ = TcpStream::connect_timeout(&SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), self.port), Duration::from_secs(1));
        if let Some(a) = self.accept.take() { let _ = a.join(); }
        std::mem::take(&mut *self.conns.lock().unwrap_or_else(|e| e.into_inner()))
    }
}
