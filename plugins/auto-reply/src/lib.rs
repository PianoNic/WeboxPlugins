//! Auto Reply: answers incoming messages in the user's own voice. An LLM gets the conversation and the user's own past
//! messages as style examples, and writes the reply the user would write (or none). Off by default; by default it only
//! suggests the reply in a toast. Uses the contract 1.6 event `message.received` and the methods `space.history` and
//! `space.send`.
//!
//! Threads: `message.received` arrives on Webox's UI thread, so it only checks the guards and starts a worker; the worker waits
//! the delay, reads the history, runs the provider (a child process) and sends or suggests through the host. `stop` sets
//! `cancel` (which kills a running child) and joins every worker before it returns.
mod i18n;
mod llm;

use collabhost_plugin_api::{export_plugin, Host, Plugin, Schema, Values};
use i18n::t;
use llm::{Llm, Msg, Provider};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How many messages of a space are read for context and style.
const HISTORY: u32 = 200;
/// Own messages from other spaces kept (in memory only) as extra style examples.
const POOL: usize = 50;

#[derive(Clone, Debug, PartialEq)]
struct Cfg {
    on: bool, llm: Llm,
    /// Also groups in which I am @-mentioned (else 1:1 chats only).
    groups: bool,
    /// "all", "allow" (only `people`) or "deny" (everyone but `people`); names compared case-insensitively.
    people_mode: String, people: Vec<String>,
    delay: Duration, cooldown: Duration,
    /// Send the reply (else suggest it in a toast).
    send: bool,
}
impl Default for Cfg {
    // The same defaults the schema declares.
    fn default() -> Self {
        Cfg { on: false, llm: Llm::default(), groups: false, people_mode: "all".into(), people: vec![], delay: Duration::from_secs(20),
            cooldown: Duration::from_secs(10 * 60), send: false }
    }
}
impl Cfg {
    fn from(v: &Values) -> Cfg {
        let d = Cfg::default();
        let t = |id: &str| v.text(id).unwrap_or("").to_string();
        Cfg {
            on: v.bool("on").unwrap_or(false),
            llm: Llm { provider: Provider::from_id(&t("provider")), cli: t("cli"), model: t("model"), endpoint: t("endpoint"), token: t("token") },
            groups: v.text("scope") == Some("mentions"),
            people_mode: v.text("people_mode").unwrap_or("all").to_string(),
            people: t("people").lines().map(|l| l.trim().to_lowercase()).filter(|l| !l.is_empty()).collect(),
            delay: v.number("delay").map_or(d.delay, |s| Duration::from_secs_f64(s.max(0.0))),
            cooldown: v.number("cooldown").map_or(d.cooldown, |m| Duration::from_secs_f64(m.max(0.0) * 60.0)),
            send: v.text("mode") == Some("send"),
        }
    }
}

/// What the UI thread and the workers share.
#[derive(Default)]
struct State {
    /// When a space last got an auto-reply (or a failed attempt): at most one per `cooldown`.
    last: HashMap<String, Instant>,
    /// Spaces with a worker on the way (a newer message there is part of its context anyway).
    pending: HashSet<String>,
    /// Replies the plugin produced: kept out of the style examples, so it does not imitate itself.
    produced: VecDeque<String>,
}

/// The `message.received` payload (the fields this plugin reads).
#[derive(Clone, Debug, Default, PartialEq)]
struct Incoming { space: String, author: String, text: String, mine: bool, direct: bool, mentioned: bool }
impl Incoming {
    fn parse(json: &str) -> Option<Incoming> {
        let v: serde_json::Value = serde_json::from_str(json).ok()?;
        let t = |k: &str| v[k].as_str().unwrap_or("").to_string();
        let b = |k: &str| v[k].as_bool().unwrap_or(false);
        let m = Incoming { space: t("space"), author: t("author"), text: t("text"), mine: b("mine"), direct: b("direct"), mentioned: b("mentioned") };
        (!m.space.is_empty()).then_some(m)
    }
}

/// Why `m` gets no auto-reply now (None = it gets one). Reasons in [`QUIET`] are not logged (they fit almost every message).
fn skip(cfg: &Cfg, st: &State, m: &Incoming, now: Instant) -> Option<&'static str> {
    if !cfg.on { return Some("switched off"); }
    if m.mine { return Some("own message"); }
    if m.text.trim().is_empty() { return Some("empty message"); }
    if !(m.direct || (cfg.groups && m.mentioned)) { return Some("not a 1:1 chat"); }
    let listed = cfg.people.iter().any(|p| *p == m.author.trim().to_lowercase());
    match cfg.people_mode.as_str() {
        "allow" if !listed => return Some("person not on the list"),
        "deny" if listed => return Some("person excluded"),
        _ => {}
    }
    if st.pending.contains(&m.space) { return Some("a reply for this chat is already on its way"); }
    if st.last.get(&m.space).is_some_and(|t| now.duration_since(*t) < cfg.cooldown) { return Some("this chat is still in its cooldown"); }
    None
}
const QUIET: &[&str] = &["switched off", "own message", "empty message", "not a 1:1 chat"];

/// Whether `m` gets an auto-reply now.
#[cfg(test)]
fn should_reply(cfg: &Cfg, st: &State, m: &Incoming, now: Instant) -> bool { skip(cfg, st, m, now).is_none() }

#[derive(Default)]
pub struct AutoReply {
    host: Option<Host>, cfg: Cfg,
    state: Arc<Mutex<State>>,
    /// Own messages seen in any space, oldest first (extra style examples; memory only).
    pool: VecDeque<String>,
    cancel: Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
}

fn lock(m: &Mutex<State>) -> std::sync::MutexGuard<'_, State> { m.lock().unwrap_or_else(|e| e.into_inner()) }

/// Waits `d` unless `cancel` is set first; true = the full time passed.
fn wait(d: Duration, cancel: &AtomicBool) -> bool {
    let end = Instant::now() + d;
    while Instant::now() < end { if cancel.load(Ordering::Relaxed) { return false; } std::thread::sleep(Duration::from_millis(100)); }
    !cancel.load(Ordering::Relaxed)
}

/// The worker for one message: wait, read the conversation, ask the model, send or suggest.
fn reply_job(host: Host, cfg: Cfg, space: String, author: String, pool: Vec<String>, state: Arc<Mutex<State>>, cancel: Arc<AtomicBool>) {
    if wait(cfg.delay, &cancel) {
        let args = serde_json::json!({ "space": space, "limit": HISTORY }).to_string();
        let hist = host.call("space.history", &args).map(|j| llm::parse_history(&j)).unwrap_or_default();
        // Every outcome is logged; in suggestion mode the person waits for a toast, so "nothing" gets one too.
        let tell = |t: &str| { host.log(t); if !cfg.send { host.toast(t); } };
        match hist.last() {
            None => tell(&t("No reply for {author}: Webox returned no history for this chat (space.history is empty).").replace("{author}", &author)),
            // I answered myself in the meantime: nothing to do.
            Some(m) if m.mine => host.log(&t("skipped ({author}): you already replied yourself").replace("{author}", &author)),
            Some(_) => {
                let produced: Vec<String> = lock(&state).produced.iter().cloned().collect();
                let prompt = llm::user_prompt(&hist, &llm::style_examples(&hist, &pool, &produced));
                let res = cfg.llm.ask(&prompt, &cancel);
                if !cancel.load(Ordering::Relaxed) {
                    match &res {
                        Ok(Some(r)) if cfg.send => {
                            let answer = host.call("space.send", &serde_json::json!({ "space": space, "text": r }).to_string()).unwrap_or_default();
                            match serde_json::from_str::<serde_json::Value>(&answer).ok().and_then(|v| v["error"].as_str().map(String::from)) {
                                Some(e) => { let m = t("Sending failed: {e}").replace("{e}", &e); host.log(&m); host.toast(&m) }
                                None => host.log(&t("sent to {author}").replace("{author}", &author)),
                            }
                        }
                        Ok(Some(r)) => {
                            host.log(&t("suggestion for {author} created").replace("{author}", &author));
                            host.toast(&t("Suggestion for {author}: {r}").replace("{author}", &author).replace("{r}", r))
                        }
                        Ok(None) => tell(&t("No reply needed ({author}): the model answered {no_reply}.").replace("{author}", &author).replace("{no_reply}", llm::NO_REPLY)),
                        // Errors always show, also when sending automatically.
                        Err(e) => { let m = t("Provider error: {e}").replace("{e}", &t(e)); host.log(&m); host.toast(&m) }
                    }
                    let mut st = lock(&state);
                    if let Ok(Some(r)) = &res { st.produced.push_back(r.clone()); if st.produced.len() > POOL { st.produced.pop_front(); } }
                    if !matches!(res, Ok(None)) { st.last.insert(space.clone(), Instant::now()); }
                }
            }
        }
    }
    lock(&state).pending.remove(&space);
}

impl AutoReply {
    fn spawn(&mut self, f: impl FnOnce() + Send + 'static) {
        self.workers.retain(|w| !w.is_finished());
        self.workers.push(std::thread::spawn(f));
    }
}

impl Plugin for AutoReply {
    const NAME: &'static str = "Auto Reply";
    const AUTHOR: &'static str = "Webox examples";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const DESCRIPTION: &'static str = env!("CARGO_PKG_DESCRIPTION");
    const EVENTS: &'static str = "message.received i18n.strings";

    fn start(&mut self, host: Host) {
        self.cancel.store(false, Ordering::Relaxed);
        i18n::sync(host);
        if !["message.received", "space.history", "space.send"].iter().all(|n| host.has(n)) {
            host.toast("This Webox version does not tell plugins about incoming messages (message.received, space.history or space.send is missing). Please update Webox.");
        }
        self.host = Some(host);
    }
    fn stop(&mut self) {
        // No thread may outlive stop: cancel (kills a running provider) and join them all.
        self.cancel.store(true, Ordering::Relaxed);
        for w in self.workers.drain(..) { let _ = w.join(); }
        self.host = None;
        *lock(&self.state) = State::default();
    }

    fn settings(&self) -> Option<Schema> {
        Some(Schema::new()
            .section("Auto Reply", "Answers incoming messages in your own style: a language model reads the conversation and your earlier messages and writes the reply you would write, or none.")
            .toggle("on", "Turn on Auto Reply", "", false)
            .choice("mode", "Mode", "A suggestion shows as a notification; \"Send automatically\" sends the reply itself.", "suggest",
                &[("suggest", "Suggest in a toast"), ("send", "Send automatically")])
            .choice("scope", "When to reply", "", "direct", &[("direct", "Only in 1:1 chats"), ("mentions", "1:1 chats, and groups when I am @-mentioned")])
            .choice("people_mode", "People", "", "all", &[("all", "Everyone"), ("allow", "Only people on the list"), ("deny", "Everyone except people on the list")])
            .multiline("people", "List", "One display name per line, as Webox shows it.", "")
            .number("delay", "Delay (s)", "How long to wait; if you reply yourself in that time, there is no auto reply.", 20.0, 0.0, 600.0, 1.0)
            .number("cooldown", "At most one auto reply per space every (min)", "Prevents endless loops with other bots.", 10.0, 1.0, 1440.0, 1.0)
            .section("Language model", "Local CLIs run without tools and with your login there.")
            .choice("provider", "Provider", "", "claude", &[("claude", "Claude Code (claude)"), ("codex", "Codex (codex)"), ("cursor", "Cursor (cursor-agent)"), ("custom", "Custom endpoint (OpenAI-compatible)")])
            .text("cli", "Program path", "Empty = claude / codex / cursor-agent / curl from the PATH.", "")
            .text("model", "Model", "Empty = the CLI's default. Required for a custom endpoint.", "")
            .text("endpoint", "Endpoint URL", "Only for a custom endpoint, e.g. https://api.openai.com/v1 (…/chat/completions is added).", "")
            .password("token", "API token", "Only for a custom endpoint. Stored in the OS keyring.")
            .button("test", "Test connection", "Asks the language model once and shows the result."))
    }
    fn settings_changed(&mut self, v: &Values) { self.cfg = Cfg::from(v); }
    fn settings_action(&mut self, id: &str) -> Option<String> {
        if id != "test" { return None; }
        let Some(host) = self.host else { return Some(t("Please turn the plugin on first.")) };
        let (llm, cancel) = (self.cfg.llm.clone(), self.cancel.clone());
        self.spawn(move || {
            let t0 = Instant::now();
            let hist = [Msg { author: "Test".into(), mine: false, text: t("Hi! Quick connection test, how are you?") }];
            let res = llm.ask(&llm::user_prompt(&hist, &[]), &cancel);
            if cancel.load(Ordering::Relaxed) { return; }
            let s = t0.elapsed().as_secs_f32();
            host.toast(&match res {
                Ok(Some(r)) => t("Connection ok ({s} s): {r}").replace("{s}", &format!("{s:.1}")).replace("{r}", &r),
                Ok(None) => t("Connection ok ({s} s), the model chose not to reply").replace("{s}", &format!("{s:.1}")),
                Err(e) => t("Connection failed: {e}").replace("{e}", &t(&e)),
            });
        });
        Some(t("Test running, the result shows as a notification."))
    }

    fn on_event(&mut self, name: &str, payload: &str) -> Option<String> {
        if name == "i18n.strings" { return Some(i18n::strings(payload)); }
        if name == "message.received" { if let Some(m) = Incoming::parse(payload) { self.on_message_received(&m) } }
        None
    }
}

impl AutoReply {
    fn on_message_received(&mut self, m: &Incoming) {
        if m.mine {
            if !m.text.trim().is_empty() { self.pool.push_back(m.text.to_string()); if self.pool.len() > POOL { self.pool.pop_front(); } }
            return;
        }
        let Some(host) = self.host else { return };
        {
            let mut st = lock(&self.state);
            if let Some(why) = skip(&self.cfg, &st, m, Instant::now()) {
                if !QUIET.contains(&why) { host.log(&t("skipped ({author}): {why}").replace("{author}", &m.author).replace("{why}", &t(why))); }
                return;
            }
            st.pending.insert(m.space.clone());
        }
        let (cfg, space, author, pool) = (self.cfg.clone(), m.space.clone(), m.author.clone(), self.pool.iter().cloned().collect());
        let (state, cancel) = (self.state.clone(), self.cancel.clone());
        self.spawn(move || reply_job(host, cfg, space, author, pool, state, cancel));
    }
}

export_plugin!(AutoReply);

#[cfg(test)]
mod i18n_tests {
    use super::*;
    #[test]
    fn every_setting_has_a_german_text() {
        let s = AutoReply::default().settings().unwrap().to_json();
        assert_eq!(i18n::untranslated(&s, &[AutoReply::NAME]), Vec::<String>::new());
    }
    #[test]
    fn the_event_answers_german_and_nothing_for_other_languages() {
        let mut p = AutoReply::default();
        assert_eq!(p.on_event("i18n.strings", r#"{"lang":"fr","fallbacks":["fr_FR","fr"]}"#).as_deref(), Some("{\"strings\":{}}"));
        assert_eq!(p.on_event("i18n.strings", r#"{"lang":"en","fallbacks":["en_US","en"]}"#).as_deref(), Some("{\"strings\":{}}"));
        let de: serde_json::Value = serde_json::from_str(&i18n::answer("de_CH")).unwrap();
        assert_eq!(de["strings"]["Test connection"], "Verbindung testen");
    }
}

#[cfg(test)]
mod tests;
