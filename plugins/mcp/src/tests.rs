use super::*;
use mcp::{call_tool, handle, tools};
use server::{auth_ok, host_ok, origin_ok, peer_ok, read_req, route, Req, Resp};
use std::io::{Read, Write};

/// Webox as the tools see it: three spaces (one read-only), messages, people; records every call and log line.
#[derive(Default)]
struct Mock { calls: Mutex<Vec<(String, Value)>>, logs: Mutex<Vec<String>>, confirm: Option<bool> }
impl Api for Mock {
    fn call(&self, method: &str, args: Value) -> Value {
        self.calls.lock().unwrap().push((method.into(), args.clone()));
        let spaces = json!([
            { "id": "s1", "title": "Projekt Alpha", "direct": false, "unread": true, "last": "17:05", "writable": true },
            { "id": "s2", "title": "Anna Berg", "direct": true, "unread": false, "last": "Mo", "writable": true },
            { "id": "s3", "title": "Ankündigungen", "direct": false, "unread": false, "last": "Mo", "writable": false }]);
        match method {
            "spaces.list" => {
                let q = args["query"].as_str().unwrap_or("").to_lowercase();
                json!({ "spaces": spaces.as_array().unwrap().iter().filter(|s| s["title"].as_str().unwrap().to_lowercase().contains(&q)).collect::<Vec<_>>() })
            }
            "space.history" => json!({ "messages": if args["space"] == "s1" { json!([{ "id": "m1", "author": "Anna", "mine": false, "text": "Geheimer Plan", "date": "2026-11-24", "time": "09:00" }]) } else { json!([]) } }),
            "messages.search" => json!({ "messages": [{ "id": "m1", "space": "s1", "text": "Plan A" }, { "id": "m2", "space": "s2", "text": "Plan B" }] }),
            "people.search" => json!({ "people": [{ "id": "anna", "name": "Anna Berg", "email": "anna@example.com", "status": "active" }] }),
            "ui.confirm" => match self.confirm { Some(ok) => json!({ "ok": ok }), None => json!({ "error": "nein" }) },
            "space.send" => json!({}),
            _ => json!({ "error": "unbekannt" }),
        }
    }
    fn log(&self, text: &str) { self.logs.lock().unwrap().push(text.into()) }
}
impl Mock {
    fn methods(&self) -> Vec<String> { self.calls.lock().unwrap().iter().map(|c| c.0.clone()).collect() }
    fn logs(&self) -> Vec<String> { self.logs.lock().unwrap().clone() }
}

fn req(id: u64, method: &str, params: Value) -> Value { json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }) }
fn tool_call(m: &Mock, p: &Policy, name: &str, args: Value) -> Value { call_tool(m, p, "Claude Code", name, &args) }
fn text(r: &Value) -> Value { serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap_or(r["content"][0]["text"].clone()) }
fn send_on() -> Policy { Policy { send: true, allow: vec![] } }

// --- JSON-RPC / MCP lifecycle ---

#[test]
fn initialize_negotiates_the_version_and_names_the_server() {
    let (m, p) = (Mock::default(), Policy::default());
    let r = handle(&m, &p, "c", &req(1, "initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "x" } }))).unwrap();
    assert_eq!(r["id"], 1);
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(r["result"]["capabilities"], json!({ "tools": { "listChanged": false } }));
    assert_eq!(r["result"]["serverInfo"]["name"], "webox-webex");
    // An unknown version gets the newest one.
    let r = handle(&m, &p, "c", &req(2, "initialize", json!({ "protocolVersion": "1999-01-01" }))).unwrap();
    assert_eq!(r["result"]["protocolVersion"], mcp::VERSIONS[0]);
}

#[test]
fn notifications_and_responses_get_no_answer_and_errors_are_json_rpc() {
    let (m, p) = (Mock::default(), Policy::default());
    assert_eq!(handle(&m, &p, "c", &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })), None);
    assert_eq!(handle(&m, &p, "c", &json!({ "jsonrpc": "2.0", "id": 5, "result": {} })), None);
    assert_eq!(handle(&m, &p, "c", &req(3, "ping", json!({})) ).unwrap()["result"], json!({}));
    assert_eq!(handle(&m, &p, "c", &req(4, "nope", json!({}))).unwrap()["error"]["code"], -32601);
    assert_eq!(handle(&m, &p, "c", &json!({ "id": 6, "method": "ping" })).unwrap()["error"]["code"], -32600, "jsonrpc 2.0 is required");
    assert_eq!(handle(&m, &p, "c", &json!([1])).unwrap()["error"]["code"], -32600);
    // String ids are echoed as they came.
    assert_eq!(handle(&m, &p, "c", &json!({ "jsonrpc": "2.0", "id": "a", "method": "ping" })).unwrap()["id"], "a");
    assert!(m.methods().is_empty(), "nothing reached Webox");
}

#[test]
fn send_message_is_listed_and_callable_only_when_allowed() {
    let m = Mock { confirm: Some(true), ..Default::default() };
    let names = |p: &Policy| tools(p).iter().map(|t| t["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(names(&Policy::default()), ["list_spaces", "read_messages", "search_messages", "search_people"]);
    assert_eq!(names(&send_on()).last().unwrap(), "send_message");
    for t in tools(&send_on()) { assert_eq!(t["inputSchema"]["type"], "object"); assert_eq!(t["annotations"]["readOnlyHint"], t["name"] != "send_message"); }
    let call = req(7, "tools/call", json!({ "name": "send_message", "arguments": { "space": "s1", "text": "Hallo" } }));
    let r = handle(&m, &Policy::default(), "c", &call).unwrap();
    assert_eq!(r["error"]["code"], -32602);
    assert!(m.methods().is_empty(), "off: neither asked nor sent");
    let r = handle(&m, &send_on(), "c", &call).unwrap();
    assert_eq!(r["result"]["isError"], false);
    assert_eq!(m.methods(), ["spaces.list", "ui.confirm", "space.send"]);
    let l = handle(&m, &Policy::default(), "c", &req(8, "tools/list", json!({}))).unwrap();
    assert_eq!(l["result"]["tools"].as_array().unwrap().len(), 4);
}

// --- Tools against a mocked Webox ---

#[test]
fn reading_tools_respect_the_space_allowlist() {
    let m = Mock::default();
    let all = Policy::default();
    assert_eq!(text(&tool_call(&m, &all, "list_spaces", json!({})))["spaces"].as_array().unwrap().len(), 3);
    assert_eq!(text(&tool_call(&m, &all, "list_spaces", json!({ "limit": 1 })))["spaces"].as_array().unwrap().len(), 1);
    let only = Policy { send: false, allow: vec!["projekt alpha".into()] };
    let l = text(&tool_call(&m, &only, "list_spaces", json!({})));
    assert_eq!(l["spaces"].as_array().unwrap().iter().map(|s| s["id"].clone()).collect::<Vec<_>>(), [json!("s1")]);
    // By title (any case) or id.
    let r = text(&tool_call(&m, &only, "read_messages", json!({ "space": "PROJEKT ALPHA", "limit": 5 })));
    assert_eq!((r["space"]["id"].as_str(), r["messages"][0]["text"].as_str()), (Some("s1"), Some("Geheimer Plan")));
    assert_eq!(m.calls.lock().unwrap().last().unwrap().1, json!({ "space": "s1", "limit": 5 }));
    // A space outside the list does not exist for the AI.
    let r = tool_call(&m, &only, "read_messages", json!({ "space": "s2" }));
    assert_eq!(r["isError"], true);
    assert!(!m.calls.lock().unwrap().iter().any(|c| c.0 == "space.history" && c.1["space"] == "s2"));
    // Search results from other spaces are dropped.
    let s = text(&tool_call(&m, &only, "search_messages", json!({ "query": "plan" })));
    assert_eq!(s["messages"].as_array().unwrap().len(), 1);
    assert_eq!(text(&tool_call(&m, &all, "search_messages", json!({ "query": "plan" })))["messages"].as_array().unwrap().len(), 2);
    assert_eq!(tool_call(&m, &all, "search_messages", json!({ "query": " " }))["isError"], true);
    assert_eq!(text(&tool_call(&m, &all, "search_people", json!({ "query": "anna" })))["people"][0]["status"], "active");
    // An empty space says why.
    assert!(text(&tool_call(&m, &all, "read_messages", json!({ "space": "Anna Berg" })))["note"].is_string());
}

#[test]
fn every_call_is_logged_without_message_text() {
    let m = Mock { confirm: Some(true), ..Default::default() };
    tool_call(&m, &Policy::default(), "read_messages", json!({ "space": "s1" }));
    tool_call(&m, &send_on(), "send_message", json!({ "space": "s2", "text": "Streng vertraulich" }));
    tool_call(&m, &Policy::default(), "read_messages", json!({ "space": "gibt es nicht" }));
    assert_eq!(m.logs(), ["Claude Code: read_messages (Projekt Alpha) -> ok", "Claude Code: send_message (Anna Berg) -> ok", "Claude Code: read_messages -> Fehler"]);
    assert!(!m.logs().iter().any(|l| l.contains("Geheimer") || l.contains("vertraulich") || l.contains("gibt es nicht")));
}

#[test]
fn sending_needs_a_confirmation_and_a_writable_space() {
    // Declined: nothing sent, the AI learns it was declined.
    let m = Mock { confirm: Some(false), ..Default::default() };
    let r = tool_call(&m, &send_on(), "send_message", json!({ "space": "Anna Berg", "text": "Hi" }));
    assert_eq!(r["isError"], true);
    assert!(r["content"][0]["text"].as_str().unwrap().contains("declined"));
    assert!(!m.methods().contains(&"space.send".to_string()));
    assert_eq!(m.logs(), ["Claude Code: send_message (Anna Berg) -> abgelehnt"]);
    // The dialog names the client, the space and the text.
    let ask = m.calls.lock().unwrap().iter().find(|c| c.0 == "ui.confirm").unwrap().1.clone();
    assert_eq!(ask["text"], "Claude Code möchte an „Anna Berg“ senden:\n\nHi");
    assert_eq!((ask["ok"].as_str(), ask["cancel"].as_str()), (Some("Senden"), Some("Ablehnen")));
    // No dialog possible (old host, error): not sent.
    let m = Mock::default();
    assert_eq!(tool_call(&m, &send_on(), "send_message", json!({ "space": "s1", "text": "Hi" }))["isError"], true);
    assert!(!m.methods().contains(&"space.send".to_string()));
    // Read-only space and empty text: not even asked.
    let m = Mock { confirm: Some(true), ..Default::default() };
    assert_eq!(tool_call(&m, &send_on(), "send_message", json!({ "space": "Ankündigungen", "text": "Hi" }))["isError"], true);
    assert_eq!(tool_call(&m, &send_on(), "send_message", json!({ "space": "s1", "text": "  " }))["isError"], true);
    assert!(!m.methods().contains(&"ui.confirm".to_string()));
    // Confirmed: sent to that space.
    tool_call(&m, &send_on(), "send_message", json!({ "space": "s1", "text": "Bis gleich" }));
    assert_eq!(m.calls.lock().unwrap().last().unwrap(), &("space.send".to_string(), json!({ "space": "s1", "text": "Bis gleich" })));
}

// --- HTTP: auth, Host / Origin, routing ---

const TOKEN: &str = "0123456789abcdef";
const PORT: u16 = 39847;
fn shared(p: Policy) -> server::Shared { Arc::new(Mutex::new(server::State { policy: p, token: TOKEN.into(), ..Default::default() })) }
fn http(method: &str, headers: &[(&str, &str)], body: &str) -> Req {
    let mut h: Vec<(String, String)> = vec![("Host".into(), format!("127.0.0.1:{PORT}")), ("Authorization".into(), format!("Bearer {TOKEN}")), ("Content-Type".into(), "application/json".into())];
    for (k, v) in headers {
        h.retain(|x| !x.0.eq_ignore_ascii_case(k));
        if !v.is_empty() { h.push((k.to_string(), v.to_string())); }
    }
    Req { method: method.into(), path: "/mcp".into(), headers: h, body: body.as_bytes().to_vec() }
}
fn run(m: &Mock, st: &server::Shared, r: &Req) -> Resp { route(m, st, PORT, r, &mut || "sid-1".to_string()) }
fn json_of(r: &Resp) -> Value { serde_json::from_str(&r.body).unwrap() }
fn hdr<'a>(r: &'a Resp, n: &str) -> Option<&'a str> { r.headers.iter().find(|h| h.0.eq_ignore_ascii_case(n)).map(|h| h.1.as_str()) }

#[test]
fn checks_for_token_host_origin_and_peer() {
    assert!(auth_ok(Some("Bearer 0123456789abcdef"), TOKEN) && auth_ok(Some("bearer 0123456789abcdef"), TOKEN));
    for bad in [None, Some(""), Some("Bearer"), Some("Bearer 0123456789abcdeX"), Some("Basic 0123456789abcdef"), Some("Bearer 0123")] { assert!(!auth_ok(bad, TOKEN), "{bad:?}"); }
    assert!(!auth_ok(Some("Bearer "), ""), "an empty token never matches");
    assert!(host_ok(Some("127.0.0.1:39847"), PORT) && host_ok(Some("localhost:39847"), PORT) && host_ok(Some("[::1]:39847"), PORT));
    for bad in [None, Some("evil.example:39847"), Some("127.0.0.1"), Some("127.0.0.1:80"), Some("127.0.0.1.evil.example:39847")] { assert!(!host_ok(bad, PORT), "{bad:?}"); }
    for good in [None, Some("http://localhost"), Some("http://127.0.0.1:6274"), Some("https://[::1]:3000")] { assert!(origin_ok(good), "{good:?}"); }
    for bad in [Some("https://evil.example"), Some("http://localhost.evil.example"), Some("null"), Some("http://127.0.0.1.nip.io"), Some("http://localhost:abc"), Some("file://")] { assert!(!origin_ok(bad), "{bad:?}"); }
    assert!(peer_ok("127.0.0.1".parse().unwrap()) && peer_ok("::1".parse().unwrap()) && peer_ok("::ffff:127.0.0.1".parse().unwrap()));
    assert!(!peer_ok("192.168.1.20".parse().unwrap()) && !peer_ok("10.0.0.1".parse().unwrap()) && !peer_ok("::ffff:192.168.1.20".parse().unwrap()));
}

#[test]
fn requests_without_token_or_from_foreign_pages_are_refused_before_any_tool_runs() {
    let (m, st) = (Mock::default(), shared(Policy::default()));
    let list = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_spaces","arguments":{}}}"#;
    let r = run(&m, &st, &http("POST", &[("Authorization", "")], list));
    assert_eq!((r.status, hdr(&r, "WWW-Authenticate")), (401, Some("Bearer")));
    assert_eq!(run(&m, &st, &http("POST", &[("Authorization", "Bearer falsch")], list)).status, 401);
    assert_eq!(run(&m, &st, &http("POST", &[("Origin", "https://evil.example")], list)).status, 403);
    assert_eq!(run(&m, &st, &http("POST", &[("Host", "evil.example:39847")], list)).status, 403);
    assert!(m.methods().is_empty(), "no tool ran");
    assert_eq!(m.logs(), ["abgewiesen: Anfrage ohne gültiges Token", "abgewiesen: Anfrage ohne gültiges Token",
        "abgewiesen: fremder Host oder fremde Herkunft (Origin)", "abgewiesen: fremder Host oder fremde Herkunft (Origin)"]);
    // The token changes in the settings: the old one stops working at once.
    st.lock().unwrap().token = "neu".into();
    assert_eq!(run(&m, &st, &http("POST", &[], list)).status, 401);
}

#[test]
fn http_routing_sessions_and_client_names() {
    let (m, st) = (Mock::default(), shared(Policy::default()));
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"LM Studio","version":"1"}}}"#;
    let r = run(&m, &st, &http("POST", &[], init));
    assert_eq!((r.status, hdr(&r, "Mcp-Session-Id"), hdr(&r, "Content-Type")), (200, Some("sid-1"), Some("application/json")));
    assert_eq!(json_of(&r)["result"]["protocolVersion"], "2025-06-18");
    // Notifications: 202 without a body.
    let r = run(&m, &st, &http("POST", &[("Mcp-Session-Id", "sid-1")], r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#));
    assert_eq!((r.status, r.body.as_str()), (202, ""));
    // The session's client name goes into the log.
    let call = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"read_messages","arguments":{"space":"s1"}}}"#;
    assert_eq!(run(&m, &st, &http("POST", &[("Mcp-Session-Id", "sid-1"), ("MCP-Protocol-Version", "2025-06-18")], call)).status, 200);
    assert_eq!(m.logs().last().unwrap(), "LM Studio: read_messages (Projekt Alpha) -> ok");
    // Without a session (lenient) / with an unknown one (the client must initialize again).
    run(&m, &st, &http("POST", &[], call));
    assert_eq!(m.logs().last().unwrap(), "MCP-Client: read_messages (Projekt Alpha) -> ok");
    assert_eq!(run(&m, &st, &http("POST", &[("Mcp-Session-Id", "alt")], call)).status, 404);
    // A batch answers as an array.
    let r = run(&m, &st, &http("POST", &[], r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/x"},{"jsonrpc":"2.0","id":2,"method":"ping"}]"#));
    assert_eq!(json_of(&r).as_array().unwrap().len(), 2);
    // Everything else.
    assert_eq!(run(&m, &st, &http("GET", &[], "")).status, 405, "no SSE stream");
    assert_eq!(run(&m, &st, &http("POST", &[("Content-Type", "text/plain")], init)).status, 415);
    assert_eq!(run(&m, &st, &http("POST", &[("MCP-Protocol-Version", "1999-01-01")], init)).status, 400);
    let r = run(&m, &st, &http("POST", &[], "{kaputt"));
    assert_eq!((r.status, json_of(&r)["error"]["code"].as_i64()), (400, Some(-32700)));
    assert_eq!(run(&m, &st, &Req { path: "/".into(), ..http("POST", &[], init) }).status, 404);
    assert_eq!(run(&m, &st, &http("DELETE", &[("Mcp-Session-Id", "sid-1")], "")).status, 200);
    assert!(st.lock().unwrap().sessions.is_empty());
}

#[test]
fn http_requests_are_parsed_with_limits() {
    let r = read_req("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:1\r\nContent-Length: 2\r\n\r\n{}".as_bytes()).unwrap();
    assert_eq!((r.method.as_str(), r.path.as_str(), r.header("host"), r.body.as_slice()), ("POST", "/mcp", Some("127.0.0.1:1"), &b"{}"[..]));
    assert_eq!(read_req("POST /mcp HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n".as_bytes()).err(), Some(411));
    assert_eq!(read_req("POST /mcp HTTP/1.1\r\nContent-Length: 99999999\r\n\r\n".as_bytes()).err(), Some(413));
    assert_eq!(read_req("POST /mcp HTTP/1.1\r\nContent-Length: 5\r\n\r\n{}".as_bytes()).err(), Some(400), "body cut short");
    assert_eq!(read_req("".as_bytes()).err(), Some(400));
    let long = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(20_000));
    assert_eq!(read_req(long.as_bytes()).err(), Some(431));
}

/// The real socket: served on 127.0.0.1, refused without the token, and `stop` returns at once.
#[test]
fn server_answers_on_loopback_and_stops() {
    let st = shared(Policy::default());
    let s = server::start(0, Arc::new(Mock::default()), st, || "sid".into()).unwrap();
    let port = s.port;
    let post = |auth: &str| {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(c, "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        out
    };
    let ok = post(&format!("Authorization: Bearer {TOKEN}\r\n"));
    assert!(ok.starts_with("HTTP/1.1 200 OK\r\n") && ok.ends_with(r#"{"id":1,"jsonrpc":"2.0","result":{}}"#), "{ok}");
    assert!(post("").starts_with("HTTP/1.1 401 "));
    let t0 = std::time::Instant::now();
    for h in s.stop() { h.join().unwrap(); }
    assert!(t0.elapsed() < std::time::Duration::from_secs(3));
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err(), "nothing listens after stop");
}

// --- Settings ---

#[test]
fn settings_and_the_connection_snippet() {
    let v = Values::parse(r#"{"active":true,"port":40000,"token":" abc ","send":false,"spaces":"Projekt Alpha\n\n  s2 \n"}"#);
    let c = Cfg::from(&v);
    assert_eq!(c, Cfg { active: true, port: 40000, token: "abc".into(), policy: Policy { send: false, allow: vec!["projekt alpha".into(), "s2".into()] } });
    assert_eq!(Cfg::from(&Values::parse("{}")), Cfg::default());
    assert_eq!(Cfg::default().port, DEFAULT_PORT);
    let s = snippet(&c);
    assert!(s.contains("claude mcp add --transport http webex http://127.0.0.1:40000/mcp --header \"Authorization: Bearer abc\""), "{s}");
    let json_part: Value = serde_json::from_str(&s[s.find('{').unwrap()..]).unwrap();
    assert_eq!(json_part["mcpServers"]["webex"], json!({ "type": "http", "url": "http://127.0.0.1:40000/mcp", "headers": { "Authorization": "Bearer abc" } }));
    // The schema has the German settings and safe defaults.
    let schema: Value = serde_json::from_str(&WebexMcp::default().settings().unwrap().to_json()).unwrap();
    let fields: Vec<Value> = schema["sections"].as_array().unwrap().iter().flat_map(|s| s["fields"].as_array().unwrap().clone()).collect();
    let f = |id: &str| fields.iter().find(|f| f["id"] == id).unwrap().clone();
    assert_eq!((f("active")["default"].clone(), f("send")["default"].clone()), (json!(false), json!(false)));
    assert_eq!((f("active")["label"].as_str(), f("send")["label"].as_str()), (Some("MCP-Server aktiv"), Some("KI darf Nachrichten senden")));
    assert_eq!(f("token")["type"], "password");
    for b in ["show_token", "new_token", "copy", "calls"] { assert_eq!(f(b)["type"], "button"); }
}

#[test]
fn tokens_are_random_hex() {
    let (a, b) = (rand::hex(32).unwrap(), rand::hex(32).unwrap());
    assert_eq!(a.len(), 64);
    assert!(a.bytes().all(|c| c.is_ascii_hexdigit()) && a != b);
}

#[test]
fn a_switched_off_plugin_listens_nowhere() {
    // Without a host (not started) nothing runs, whatever the settings say.
    let mut p = WebexMcp::default();
    p.settings_changed(&Values::parse(r#"{"active":true,"port":0,"token":"abc"}"#));
    assert!(p.server.is_none());
    assert_eq!(p.settings_action("copy").as_deref(), Some("Bitte das Plugin zuerst einschalten."));
    assert_eq!(p.settings_action("calls").as_deref(), Some("Noch keine Aufrufe."));
}
