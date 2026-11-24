use super::*;
use llm::{clean, parse_history, style_examples, user_prompt, Invocation};

fn m(author: &str, mine: bool, text: &str) -> Msg { Msg { author: author.into(), mine, text: text.into() } }
const DIRECT: u32 = 1;
const MINE: u32 = 2;
const MENTIONED: u32 = 4;
fn nm(space: &str, author: &str, text: &str, f: u32) -> Incoming {
    Incoming { space: space.into(), author: author.into(), text: text.into(), direct: f & DIRECT != 0, mine: f & MINE != 0, mentioned: f & MENTIONED != 0 }
}
fn on() -> Cfg { Cfg { on: true, ..Cfg::default() } }
fn llm(p: Provider) -> Llm { Llm { provider: p, ..Llm::default() } }

#[test]
fn history_json_is_read_and_bad_entries_dropped() {
    let h = parse_history(r#"{"messages":[{"author":"Anna","mine":false,"text":"Hi"},{"author":"Ich","mine":true,"text":"Hey"},{"author":1}]}"#);
    assert_eq!(h, vec![m("Anna", false, "Hi"), m("Ich", true, "Hey")]);
    assert!(parse_history("kein json").is_empty());
}

#[test]
fn message_received_payload_is_read() {
    let m = Incoming::parse(r#"{"space":"s1","id":"m1","author":"Anna","text":"Hi","mine":false,"direct":true,"mentioned":false,"later":1}"#);
    assert_eq!(m, Some(nm("s1", "Anna", "Hi", DIRECT)));
    assert_eq!(Incoming::parse(r#"{"author":"Anna"}"#), None);
    assert_eq!(Incoming::parse("kaputt"), None);
    // An unreadable payload is ignored, and the event never answers.
    assert_eq!(AutoReply::default().on_event("message.received", "kaputt"), None);
}

#[test]
fn style_examples_prefer_this_chat_skip_repeats_and_own_replies() {
    let hist = vec![m("Ich", true, "jo passt"), m("Anna", false, "Kommst du?"), m("Ich", true, "bin gleich da 👍"), m("Ich", true, "jo passt"), m("Ich", true, "  ")];
    let pool = vec!["älter".to_string(), "Moin!".to_string(), "bin gleich da 👍".to_string()];
    let produced = vec!["Moin!".to_string()];
    // Newest first: this conversation, then the pool (newest first), no repeats, no empty ones, no own auto-replies.
    assert_eq!(style_examples(&hist, &pool, &produced), vec!["jo passt", "bin gleich da 👍", "älter"]);
    let many: Vec<Msg> = (0..60).map(|i| m("Ich", true, &format!("n{i}"))).collect();
    let s = style_examples(&many, &[], &[]);
    assert_eq!((s.len(), s[0].as_str()), (llm::STYLE, "n59"));
}

#[test]
fn prompt_has_style_context_and_the_message_to_answer() {
    let mut hist: Vec<Msg> = (0..40).map(|i| m("Anna", false, &format!("alt{i}"))).collect();
    hist.push(m("Ich", true, "ok mach ich"));
    hist.push(m("Anna", false, "Und morgen\num 9?"));
    let p = user_prompt(&hist, &["jo passt".into()]);
    assert!(p.contains("- jo passt\n"));
    assert!(p.contains("USER: ok mach ich\nAnna: Und morgen\n  um 9?\n"), "{p}");
    // Only the last 30 messages as context.
    assert!(!p.contains("alt11\n") && p.contains("alt12\n"));
    assert!(p.ends_with("Write USER's reply to Anna's last message, or NO_REPLY."));
    assert!(user_prompt(&[m("A", false, "x")], &[]).contains("(none yet"));
    // The system prompt asks for the things the reply depends on.
    for w in ["language of the last incoming message", "du/Sie", "emoji", "NO_REPLY", "message text only"] { assert!(llm::SYSTEM.contains(w), "{w}"); }
}

#[test]
fn answers_are_cleaned_and_no_reply_respected() {
    assert_eq!(clean("  \"Klar, bis gleich!\"\n").as_deref(), Some("Klar, bis gleich!"));
    assert_eq!(clean("USER: jo").as_deref(), Some("jo"));
    assert_eq!(clean("NO_REPLY"), None);
    assert_eq!(clean("  "), None);
}

#[test]
fn guards_skip_own_groups_people_pending_and_cooldown() {
    let now = Instant::now();
    let st = State::default();
    let dm = nm("s1", "Anna", "Hast du kurz Zeit?", DIRECT);
    assert!(should_reply(&on(), &st, &dm, now));
    assert!(!should_reply(&Cfg::default(), &st, &dm, now), "off by default");
    assert!(!should_reply(&on(), &st, &nm("s1", "Ich", "hi", DIRECT | MINE), now), "never my own message");
    assert!(!should_reply(&on(), &st, &nm("s1", "Anna", " ", DIRECT), now));
    // Groups: only with the scope set and only when mentioned.
    let group = nm("g", "Anna", "@Ich schaust du?", MENTIONED);
    assert!(!should_reply(&on(), &st, &group, now));
    let groups = Cfg { groups: true, ..on() };
    assert!(should_reply(&groups, &st, &group, now));
    assert!(!should_reply(&groups, &st, &nm("g", "Anna", "an alle", 0), now));
    // Allow / deny list, by display name, case-insensitive.
    let allow = Cfg { people_mode: "allow".into(), people: vec!["anna".into()], ..on() };
    assert!(should_reply(&allow, &st, &dm, now));
    assert!(!should_reply(&allow, &st, &nm("s2", "Bob", "hi", DIRECT), now));
    let deny = Cfg { people_mode: "deny".into(), ..allow };
    assert!(!should_reply(&deny, &st, &dm, now));
    // One worker per space; then at most one reply per cooldown (another bot answering cannot start a chain).
    let busy = State { pending: ["s1".to_string()].into(), ..State::default() };
    assert!(!should_reply(&on(), &busy, &dm, now));
    let recent = State { last: [("s1".to_string(), now)].into(), ..State::default() };
    assert!(!should_reply(&on(), &recent, &dm, now + Duration::from_secs(60)));
    assert!(should_reply(&on(), &recent, &dm, now + Duration::from_secs(601)));
    assert!(should_reply(&on(), &recent, &nm("s2", "Anna", "hi", DIRECT), now), "other spaces are not limited");
}

#[test]
fn settings_are_read_with_their_defaults() {
    let p = AutoReply::default();
    let schema = p.settings().unwrap().to_json();
    for id in ["\"on\"", "\"mode\"", "\"scope\"", "\"provider\"", "\"cli\"", "\"token\"", "\"test\""] { assert!(schema.contains(id), "{id}"); }
    assert_eq!(Cfg::from(&Values::parse("{}")), Cfg::default());
    let c = Cfg::from(&Values::parse(r#"{"on":true,"mode":"send","scope":"mentions","provider":"custom","people":"Anna\n\n BOB ","people_mode":"deny","delay":5,"cooldown":2,"token":"t"}"#));
    assert!(c.on && c.send && c.groups);
    assert_eq!((c.llm.provider, c.llm.token.as_str()), (Provider::Custom, "t"));
    assert_eq!(c.people, vec!["anna", "bob"]);
    assert_eq!((c.delay, c.cooldown), (Duration::from_secs(5), Duration::from_secs(120)));
}

#[test]
fn claude_invocation_and_output() {
    let l = Llm { model: "sonnet".into(), ..llm(Provider::Claude) };
    let i = l.invocation("PROMPT").unwrap();
    assert_eq!(i.program, "claude");
    assert_eq!(i.stdin, "PROMPT");
    let a = i.args.join(" ");
    assert!(a.starts_with("-p --output-format json --tools  --system-prompt You write") && a.ends_with("--setting-sources  --model sonnet"), "{a}");
    let ok = r#"{"type":"result","subtype":"success","is_error":false,"result":"Klar, bis gleich!","total_cost_usd":0.005}"#;
    assert_eq!(l.parse(true, ok, ""), Ok(Some("Klar, bis gleich!".into())));
    assert_eq!(l.parse(true, &ok.replace("Klar, bis gleich!", "NO_REPLY"), ""), Ok(None));
    assert_eq!(l.parse(false, r#"{"type":"result","is_error":true,"result":"Not logged in · Please run /login"}"#, ""), Err("Not logged in · Please run /login".into()));
    assert_eq!(l.parse(false, "", "command not found"), Err("command not found".into()));
    // A configured path wins.
    assert_eq!(Llm { cli: r" C:\Tools\claude.exe ".into(), ..llm(Provider::Claude) }.invocation("x").unwrap().program, r"C:\Tools\claude.exe");
}

#[test]
fn codex_invocation_and_output() {
    let l = Llm { model: "gpt-5".into(), ..llm(Provider::Codex) };
    let i = l.invocation("PROMPT").unwrap();
    assert_eq!(i.args, ["exec", "--skip-git-repo-check", "--sandbox", "read-only", "--color", "never", "-m", "gpt-5", "-"]);
    assert!(i.stdin.starts_with(llm::SYSTEM) && i.stdin.ends_with("\n\nPROMPT"));
    assert_eq!(l.parse(true, "Passt, danke!\n", "[progress on stderr]"), Ok(Some("Passt, danke!".into())));
    assert_eq!(l.parse(true, "NO_REPLY\n", ""), Ok(None));
    assert_eq!(l.parse(false, "", "error: not logged in"), Err("error: not logged in".into()));
}

#[test]
fn cursor_invocation_and_output() {
    let l = llm(Provider::Cursor);
    let i = l.invocation("PROMPT").unwrap();
    assert_eq!((i.program.as_str(), &i.args[..3]), ("cursor-agent", &["-p".to_string(), "--output-format".into(), "json".into()][..]));
    assert!(i.args.last().unwrap().ends_with("\n\nPROMPT") && i.stdin.is_empty());
    let out = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":1800,"result":"Ja, passt 👍","session_id":"x"}"#;
    assert_eq!(l.parse(true, out, ""), Ok(Some("Ja, passt 👍".into())));
}

#[test]
fn custom_endpoint_goes_through_curl_with_the_token_on_stdin() {
    let l = Llm { endpoint: "https://api.example.com/v1/".into(), model: "gpt-4o-mini".into(), token: "sk-\"geheim\"".into(), ..llm(Provider::Custom) };
    let Invocation { program, args, stdin } = l.invocation("Hallo \"du\"\nzweite Zeile").unwrap();
    assert_eq!((program.as_str(), args.join(" ")), ("curl", "-sS --max-time 120 -w \\n%{http_code} -K -".to_string()));
    assert!(!args.iter().any(|a| a.contains("sk-")), "the token never goes on the command line");
    assert!(stdin.contains("url = \"https://api.example.com/v1/chat/completions\"\n"));
    assert!(stdin.contains("header = \"Authorization: Bearer sk-\\\"geheim\\\"\"\n"), "{stdin}");
    // The body, unquoted again, is the JSON request.
    let body = stdin.lines().find_map(|l| l.strip_prefix("data-binary = \"")).unwrap().trim_end_matches('"').replace("\\\"", "\"").replace("\\\\", "\\");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!((v["model"].as_str(), v["messages"][1]["content"].as_str()), (Some("gpt-4o-mini"), Some("Hallo \"du\"\nzweite Zeile")));
    assert_eq!(v["messages"][0]["role"], "system");
    let ok = r#"{"id":"c","choices":[{"index":0,"message":{"role":"assistant","content":"Moin, klar!"},"finish_reason":"stop"}]}"#;
    assert_eq!(l.parse(true, ok, ""), Ok(Some("Moin, klar!".into())));
    assert_eq!(l.parse(true, r#"{"error":{"message":"Incorrect API key provided"}}"#, ""), Err("Incorrect API key provided".into()));
    assert_eq!(l.parse(false, "", "curl: (6) Could not resolve host"), Err("curl: (6) Could not resolve host".into()));
    assert!(Llm { endpoint: "".into(), ..l.clone() }.invocation("x").is_err());
    assert!(Llm { model: "".into(), ..l }.invocation("x").is_err());
}

#[test]
fn custom_endpoint_errors_say_the_http_status() {
    let l = Llm { endpoint: "https://api.example.com/v1".into(), model: "m".into(), ..llm(Provider::Custom) };
    // `-w` puts the status on the last line.
    let ok = "{\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"hoi!\"},\"finish_reason\":\"stop\"}]}\n200";
    assert_eq!(l.parse(true, ok, ""), Ok(Some("hoi!".into())));
    assert_eq!(l.parse(true, &ok.replace("hoi!", "NO_REPLY"), ""), Ok(None));
    assert_eq!(l.parse(true, "{\"error\":{\"message\":\"Invalid API key\"}}\n401", ""), Err("HTTP 401: Invalid API key".into()));
    assert_eq!(l.parse(true, "{\"error\":\"model not found\"}\n404", ""), Err("HTTP 404: model not found".into()));
    assert_eq!(l.parse(true, "<html>Bad Gateway</html>\n502", ""), Err("HTTP 502: <html>Bad Gateway</html>".into()));
    assert_eq!(l.parse(true, "\n503", ""), Err("HTTP 503".into()));
    assert_eq!(l.parse(false, "\n000", "curl: (28) Operation timed out"), Err("curl: (28) Operation timed out".into()));
}

#[test]
fn reasoning_and_empty_answers() {
    let l = Llm { endpoint: "https://x/v1".into(), model: "m".into(), ..llm(Provider::Custom) };
    let with = |content: &str| format!("{{\"choices\":[{{\"message\":{{\"content\":{content}}},\"finish_reason\":\"length\"}}]}}\n200");
    assert_eq!(l.parse(true, &with("\"<think>Ein Gruss.</think>\\n\\nhoi zäme\""), ""), Ok(Some("hoi zäme".into())));
    assert_eq!(l.parse(true, &with("[{\"type\":\"text\",\"text\":\"hoi\"}]"), ""), Ok(Some("hoi".into())));
    // No text (only reasoning, cut off, null): an error with the reason, not a silent "no reply".
    assert_eq!(l.parse(true, &with("null"), ""), Err("leere Antwort des Modells (finish_reason: length)".into()));
    assert_eq!(l.parse(true, &with("\"<think>noch am Überlegen\""), ""), Err("leere Antwort des Modells (finish_reason: length)".into()));
    assert_eq!(clean("<think>x</think>NO_REPLY"), None);
}

#[test]
fn greetings_are_not_no_reply() {
    // The model answered NO_REPLY to "hoi": the instructions now say a greeting always gets a reply.
    for w in ["A greeting", "\"hoi\"", "always gets a reply", "only when"] { assert!(llm::SYSTEM.contains(w), "{w}"); }
}

#[test]
fn skipped_messages_say_why() {
    let now = Instant::now();
    let dm = nm("s1", "Anna", "hoi", DIRECT);
    assert_eq!(skip(&on(), &State::default(), &dm, now), None);
    let busy = State { pending: ["s1".to_string()].into(), ..State::default() };
    assert_eq!(skip(&on(), &busy, &dm, now), Some("für diesen Chat läuft schon eine Antwort"));
    let recent = State { last: [("s1".to_string(), now)].into(), ..State::default() };
    assert_eq!(skip(&on(), &recent, &dm, now), Some("Abklingzeit für diesen Chat läuft noch"));
    // The everyday reasons are not logged.
    assert!(QUIET.contains(&skip(&on(), &State::default(), &nm("g", "Anna", "x", 0), now).unwrap()));
}

/// A host for the worker tests: records toasts and logs, answers `host.capabilities` and `space.history`.
mod fake_host {
    use collabhost_plugin_api::{cs, HostApi};
    use std::ffi::{c_char, c_void, CStr, CString};
    use std::sync::Mutex;
    pub static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
    fn s(p: *const c_char) -> String { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
    extern "C" fn toast(_: *mut c_void, t: *const c_char) { SEEN.lock().unwrap().push(format!("toast: {}", s(t))) }
    extern "C" fn log(_: *mut c_void, t: *const c_char) { SEEN.lock().unwrap().push(format!("log: {}", s(t))) }
    extern "C" fn send(_: *mut c_void, t: *const c_char) { SEEN.lock().unwrap().push(format!("send: {}", s(t))) }
    extern "C" fn free(p: *mut c_char) { drop(unsafe { CString::from_raw(p) }) }
    extern "C" fn call(_: *mut c_void, m: *const c_char, j: *const c_char) -> *mut c_char {
        let r = match s(m).as_str() {
            "host.capabilities" => r#"{"message.received":1,"space.history":1,"space.send":1}"#.to_string(),
            "space.history" => r#"{"messages":[{"author":"Manuel","mine":false,"text":"hoi"},{"author":"Manuel","mine":false,"text":"hoi 2"}]}"#.to_string(),
            "space.send" => { SEEN.lock().unwrap().push(format!("space.send: {}", s(j))); "{}".to_string() }
            _ => return std::ptr::null_mut(),
        };
        cs(&r).into_raw()
    }
    pub fn api() -> HostApi {
        HostApi { ctx: std::ptr::null_mut(), toast: Some(toast), send: Some(send), log: Some(log), data_get: None, data_set: None,
            free_str: Some(free), settings_get: None, call: Some(call) }
    }
}

/// Answers one HTTP request on 127.0.0.1 with `status` and `body`; returns the endpoint URL.
fn one_shot_server(status: &str, body: &str) -> String {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", l.local_addr().unwrap());
    let resp = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    std::thread::spawn(move || {
        let (mut c, _) = l.accept().unwrap();
        let (mut req, mut buf) = (Vec::new(), [0u8; 4096]);
        // Headers, then as many body bytes as Content-Length says.
        loop {
            let n = c.read(&mut buf).unwrap_or(0);
            if n == 0 { break; }
            req.extend_from_slice(&buf[..n]);
            let t = String::from_utf8_lossy(&req).to_string();
            if let Some(h) = t.find("\r\n\r\n") {
                let len = t[..h].lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                if req.len() >= h + 4 + len { break; }
            }
        }
        let _ = c.write_all(resp.as_bytes());
    });
    url
}

/// The worker end to end through the plugin's table, a fake host and a local endpoint (real curl): what the person sees.
#[test]
fn every_outcome_is_shown_in_suggestion_mode() {
    if std::process::Command::new("curl").arg("--version").output().is_err() { eprintln!("no curl; skipping"); return; }
    let v = unsafe { &*collabhost_plugin_api::vtable::<AutoReply>() };
    let api = fake_host::api();
    let run = |status: &str, body: &str, space: &str| -> Vec<String> {
        let url = one_shot_server(status, body);
        let settings = serde_json::json!({ "on": true, "mode": "suggest", "delay": 0, "provider": "custom", "endpoint": url, "model": "m" }).to_string();
        let c = collabhost_plugin_api::cs(&settings);
        (v.settings_changed.unwrap())(v.instance, c.as_ptr());
        fake_host::SEEN.lock().unwrap().clear();
        let (n, p) = (collabhost_plugin_api::cs("message.received"),
            collabhost_plugin_api::cs(&serde_json::json!({ "space": space, "id": "m1", "author": "Manuel", "text": "hoi 2", "mine": false, "direct": true, "mentioned": false }).to_string()));
        (v.on_event.unwrap())(v.instance, n.as_ptr(), p.as_ptr());
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(20) && !fake_host::SEEN.lock().unwrap().iter().any(|l| l.starts_with("toast")) {
            std::thread::sleep(Duration::from_millis(50));
        }
        fake_host::SEEN.lock().unwrap().clone()
    };
    (v.start.unwrap())(v.instance, &api);
    let ok = |c: &str| format!("{{\"choices\":[{{\"message\":{{\"content\":\"{c}\"}},\"finish_reason\":\"stop\"}}]}}");
    let seen = run("200 OK", &ok("hoi Manuel!"), "a");
    assert!(seen.contains(&"toast: Vorschlag für Manuel: hoi Manuel!".to_string()), "{seen:?}");
    let seen = run("200 OK", &ok("NO_REPLY"), "b");
    assert!(seen.contains(&"toast: Keine Antwort nötig (Manuel): das Modell hat NO_REPLY geantwortet.".to_string()), "{seen:?}");
    let seen = run("401 Unauthorized", r#"{"error":{"message":"Invalid API key"}}"#, "c");
    assert!(seen.contains(&"toast: Anbieter-Fehler: HTTP 401: Invalid API key".to_string()), "{seen:?}");
    assert!(seen.contains(&"log: Anbieter-Fehler: HTTP 401: Invalid API key".to_string()), "{seen:?}");
    assert!(!seen.iter().any(|l| l.contains("send")), "suggestion mode never sends");
    (v.stop.unwrap())(v.instance);
    (v.destroy.unwrap())(v.instance);
}

#[test]
fn a_running_provider_is_killed_on_cancel() {
    // A stand-in child that would run for 30 s; `stop` sets cancel and the run must end at once.
    let inv = if cfg!(windows) { Invocation { program: "ping".into(), args: vec!["-n".into(), "30".into(), "127.0.0.1".into()], stdin: String::new() } }
              else { Invocation { program: "sleep".into(), args: vec!["30".into()], stdin: String::new() } };
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    let t = std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(300)); c.store(true, Ordering::Relaxed) });
    let t0 = Instant::now();
    assert_eq!(llm::run(&inv, &cancel), Err("abgebrochen".into()));
    assert!(t0.elapsed() < Duration::from_secs(5));
    t.join().unwrap();
    assert!(llm::run(&Invocation { program: "webox-no-such-cli".into(), args: vec![], stdin: String::new() }, &AtomicBool::new(false)).is_err());
}
