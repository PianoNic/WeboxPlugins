//! The prompt, and running it through a provider: a local agent CLI (Claude Code, Codex, Cursor) or an OpenAI-compatible
//! chat-completions endpoint (through `curl`, so a request can be killed on `stop` like the CLIs). Every provider runs as a
//! child process; nothing here touches the host.
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// What the model answers when no reply fits.
pub const NO_REPLY: &str = "NO_REPLY";
/// How long one provider run may take.
pub const TIMEOUT: Duration = Duration::from_secs(120);
/// Messages of the conversation given as context, and own messages given as style examples.
pub const CONTEXT: usize = 30;
pub const STYLE: usize = 25;
/// Longer messages are cut (keeps the prompt well under the Windows command-line limit for Cursor).
const MSG_CHARS: usize = 400;

pub const SYSTEM: &str = "You write chat replies on behalf of the user, in the user's own voice. \
Reply with the message text only: no quotes, no name or label in front, no explanations. \
Write in the language of the last incoming message. \
Imitate the user's style from their example messages: length, tone, formality (du/Sie, tu/vous), greetings, emoji use, \
punctuation, capitalisation and typical phrases. Keep it as short as the user usually writes. \
Do not invent facts, appointments or promises; if a real answer needs something only the user knows, reply briefly and \
noncommittally the way the user would. \
If no reply is appropriate (for example the message is just \"ok\", a thanks, an emoji or a reaction, or the conversation \
is finished), answer with exactly NO_REPLY.";

/// One message as the host's `space.history` returns it.
#[derive(Clone, Debug, PartialEq)]
pub struct Msg { pub author: String, pub mine: bool, pub text: String }

/// The answer of `space.history` (`{"messages": [{"author","mine","text"}]}`, oldest first); anything unreadable is left out.
pub fn parse_history(json: &str) -> Vec<Msg> {
    let v: Value = serde_json::from_str(json).unwrap_or_default();
    v["messages"].as_array().into_iter().flatten().filter_map(|m| Some(Msg {
        author: m["author"].as_str()?.to_string(), mine: m["mine"].as_bool()?, text: m["text"].as_str()?.to_string(),
    })).collect()
}

/// Up to [`STYLE`] of the user's own messages, newest first: those of this conversation, then `pool` (own messages seen in
/// other spaces, oldest first). Empty ones, repeats and the plugin's own replies (`exclude`) are left out.
pub fn style_examples(hist: &[Msg], pool: &[String], exclude: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let here = hist.iter().rev().filter(|m| m.mine).map(|m| m.text.trim());
    for t in here.chain(pool.iter().rev().map(|t| t.trim())) {
        if out.len() >= STYLE { break; }
        if t.is_empty() || exclude.iter().any(|x| x.trim() == t) || out.iter().any(|x| x == t) { continue; }
        out.push(t.to_string());
    }
    out
}

fn cut(t: &str) -> String {
    let t = t.trim();
    let mut s: String = t.chars().take(MSG_CHARS).collect();
    if s.len() < t.len() { s.push('…'); }
    s.replace('\n', "\n  ")
}

/// The user part of the prompt: the style examples and the last [`CONTEXT`] messages, ending with the one to answer.
pub fn user_prompt(hist: &[Msg], style: &[String]) -> String {
    let mut p = String::from("Messages the user wrote earlier (for style only, not context):\n");
    if style.is_empty() { p.push_str("(none yet: write short, friendly and natural)\n"); }
    for t in style { p.push_str(&format!("- {}\n", cut(t))); }
    p.push_str("\nThe conversation, oldest first. \"USER\" is the user you write for:\n");
    for m in &hist[hist.len().saturating_sub(CONTEXT)..] {
        p.push_str(&format!("{}: {}\n", if m.mine { "USER" } else { m.author.as_str() }, cut(&m.text)));
    }
    let to = hist.iter().rev().find(|m| !m.mine).map_or("the other person", |m| m.author.as_str());
    p.push_str(&format!("\nWrite USER's reply to {to}'s last message, or {NO_REPLY}."));
    p
}

/// The reply in a model's answer: None when it said [`NO_REPLY`] or nothing.
pub fn clean(answer: &str) -> Option<String> {
    let t = answer.trim();
    let t = t.strip_prefix("USER:").unwrap_or(t).trim();
    let t = if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') { &t[1..t.len() - 1] } else { t };
    let t = t.trim();
    (!t.is_empty() && !t.contains(NO_REPLY)).then(|| t.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Provider { #[default] Claude, Codex, Cursor, Custom }
impl Provider {
    pub fn from_id(id: &str) -> Provider {
        match id { "codex" => Provider::Codex, "cursor" => Provider::Cursor, "custom" => Provider::Custom, _ => Provider::Claude }
    }
    pub fn program(self) -> &'static str {
        match self { Provider::Claude => "claude", Provider::Codex => "codex", Provider::Cursor => "cursor-agent", Provider::Custom => "curl" }
    }
}

/// How to reach the model (from the settings).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Llm { pub provider: Provider, pub cli: String, pub model: String, pub endpoint: String, pub token: String }

/// A child process to run: program, arguments and what goes to its stdin.
#[derive(Debug, PartialEq)]
pub struct Invocation { pub program: String, pub args: Vec<String>, pub stdin: String }

/// A string in a curl config file.
fn curl_quote(s: &str) -> String { format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "")) }

impl Llm {
    /// The command for `prompt`. The prompt goes through stdin where the CLI reads it from there, so it never shows up in the
    /// process list; the token goes into curl's config on stdin for the same reason.
    pub fn invocation(&self, prompt: &str) -> Result<Invocation, String> {
        let program = if self.cli.trim().is_empty() { self.provider.program().to_string() } else { self.cli.trim().to_string() };
        let model = self.model.trim();
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (mut args, stdin) = match self.provider {
            // Print mode, one JSON result; no tools, no MCP servers, no settings, hooks or CLAUDE.md of the user's setup (they
            // would also add their context to every request), nothing saved.
            Provider::Claude => (a(&["-p", "--output-format", "json", "--tools", "", "--system-prompt", SYSTEM, "--no-session-persistence",
                "--strict-mcp-config", "--disable-slash-commands", "--setting-sources", ""]), prompt.to_string()),
            // `exec` prints only the final message on stdout (progress goes to stderr); `-` reads the prompt from stdin.
            Provider::Codex => (a(&["exec", "--skip-git-repo-check", "--sandbox", "read-only", "--color", "never"]), format!("{SYSTEM}\n\n{prompt}")),
            // Print mode with one JSON result; the prompt is the last argument.
            Provider::Cursor => (a(&["-p", "--output-format", "json"]), String::new()),
            Provider::Custom => {
                let url = self.endpoint.trim().trim_end_matches('/');
                if !url.starts_with("http://") && !url.starts_with("https://") { return Err("Endpunkt-URL fehlt".into()); }
                if model.is_empty() { return Err("Modellname fehlt".into()); }
                let url = if url.ends_with("/chat/completions") { url.to_string() } else { format!("{url}/chat/completions") };
                let body = json!({ "model": model, "messages": [
                    { "role": "system", "content": SYSTEM }, { "role": "user", "content": prompt } ] });
                let mut cfg = format!("url = {}\nheader = \"Content-Type: application/json\"\ndata-binary = {}\n", curl_quote(&url), curl_quote(&body.to_string()));
                if !self.token.is_empty() { cfg.push_str(&format!("header = {}\n", curl_quote(&format!("Authorization: Bearer {}", self.token)))); }
                (a(&["-sS", "--max-time", &TIMEOUT.as_secs().to_string(), "-K", "-"]), cfg)
            }
        };
        if !model.is_empty() && self.provider != Provider::Custom {
            args.extend(a(&[if self.provider == Provider::Codex { "-m" } else { "--model" }, model]));
        }
        match self.provider {
            Provider::Codex => args.push("-".into()),
            Provider::Cursor => args.push(format!("{SYSTEM}\n\n{prompt}")),
            _ => {}
        }
        Ok(Invocation { program, args, stdin })
    }

    /// The reply in a finished run's output (`ok` = exit status 0): Ok(None) = the model chose not to reply.
    pub fn parse(&self, ok: bool, out: &str, err: &str) -> Result<Option<String>, String> {
        let fail = |m: &str| { let m = m.trim(); Err(if m.is_empty() { "kein Ergebnis".to_string() } else { m.chars().take(300).collect() }) };
        match self.provider {
            Provider::Claude | Provider::Cursor => {
                let v: Value = serde_json::from_str(out.trim()).unwrap_or_default();
                match v["result"].as_str() {
                    Some(r) if ok && !v["is_error"].as_bool().unwrap_or(false) => Ok(clean(r)),
                    Some(r) => fail(r),
                    None => fail(if err.trim().is_empty() { out } else { err }),
                }
            }
            Provider::Codex => if ok { Ok(clean(out)) } else { fail(err) },
            Provider::Custom => {
                let v: Value = serde_json::from_str(out.trim()).unwrap_or_default();
                if let Some(c) = v["choices"][0]["message"]["content"].as_str() { return Ok(clean(c)); }
                fail(v["error"]["message"].as_str().or(v["error"].as_str()).unwrap_or(if err.trim().is_empty() { out } else { err }))
            }
        }
    }

    /// Runs the provider for `prompt`; gives up when `cancel` is set (`stop`) or after [`TIMEOUT`].
    pub fn ask(&self, prompt: &str, cancel: &AtomicBool) -> Result<Option<String>, String> {
        let inv = self.invocation(prompt)?;
        let (ok, out, err) = run(&inv, cancel)?;
        self.parse(ok, &out, &err)
    }
}

/// Runs `inv` in the temp folder (so a CLI picks up no project files) and returns (exit status 0, stdout, stderr). The child is
/// killed on `cancel` or after [`TIMEOUT`]; all helper threads are joined before this returns.
pub fn run(inv: &Invocation, cancel: &AtomicBool) -> Result<(bool, String, String), String> {
    let mut cmd = Command::new(&inv.program);
    cmd.args(&inv.args).current_dir(std::env::temp_dir()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    { use std::os::windows::process::CommandExt; cmd.creation_flags(0x0800_0000); } // CREATE_NO_WINDOW: no console flashing up
    let mut child = cmd.spawn().map_err(|e| format!("{} lässt sich nicht starten: {e}", inv.program))?;
    let (stdin, mut so, mut se) = (child.stdin.take(), child.stdout.take().unwrap(), child.stderr.take().unwrap());
    std::thread::scope(|s| {
        s.spawn(move || if let Some(mut i) = stdin { let _ = i.write_all(inv.stdin.as_bytes()); });
        let out = s.spawn(move || { let mut b = vec![]; let _ = so.read_to_end(&mut b); String::from_utf8_lossy(&b).into_owned() });
        let err = s.spawn(move || { let mut b = vec![]; let _ = se.read_to_end(&mut b); String::from_utf8_lossy(&b).into_owned() });
        let t0 = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break Ok(st.success()),
                Ok(None) if cancel.load(Ordering::Relaxed) => break Err("abgebrochen".to_string()),
                Ok(None) if t0.elapsed() > TIMEOUT => break Err(format!("keine Antwort nach {} s", TIMEOUT.as_secs())),
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => break Err(e.to_string()),
            }
        };
        if status.is_err() { let _ = child.kill(); let _ = child.wait(); }
        let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
        status.map(|ok| (ok, out, err))
    })
}
