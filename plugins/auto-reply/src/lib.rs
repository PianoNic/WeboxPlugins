//! Auto-Antwort: answers incoming messages in the user's own voice. An LLM gets the conversation and the user's own past
//! messages as style examples, and writes the reply the user would write (or none). Off by default; by default it only
//! suggests the reply in a toast. Uses the contract 1.6 event `message.received` and the methods `space.history` and
//! `space.send`.
//!
//! Threads: `message.received` arrives on Webox's UI thread, so it only checks the guards and starts a worker; the worker waits
//! the delay, reads the history, runs the provider (a child process) and sends or suggests through the host. `stop` sets
//! `cancel` (which kills a running child) and joins every worker before it returns.
mod llm;

use collabhost_plugin_api::{export_plugin, Host, Plugin, Schema, Values};
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
    if !cfg.on { return Some("ausgeschaltet"); }
    if m.mine { return Some("eigene Nachricht"); }
    if m.text.trim().is_empty() { return Some("leere Nachricht"); }
    if !(m.direct || (cfg.groups && m.mentioned)) { return Some("kein 1:1-Chat"); }
    let listed = cfg.people.iter().any(|p| *p == m.author.trim().to_lowercase());
    match cfg.people_mode.as_str() {
        "allow" if !listed => return Some("Person nicht in der Liste"),
        "deny" if listed => return Some("Person ausgeschlossen"),
        _ => {}
    }
    if st.pending.contains(&m.space) { return Some("für diesen Chat läuft schon eine Antwort"); }
    if st.last.get(&m.space).is_some_and(|t| now.duration_since(*t) < cfg.cooldown) { return Some("Abklingzeit für diesen Chat läuft noch"); }
    None
}
const QUIET: &[&str] = &["ausgeschaltet", "eigene Nachricht", "leere Nachricht", "kein 1:1-Chat"];

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
            None => tell(&format!("Keine Antwort für {author}: Webox hat keinen Verlauf dieses Chats geliefert (space.history leer).")),
            // I answered myself in the meantime: nothing to do.
            Some(m) if m.mine => host.log(&format!("übersprungen ({author}): schon selbst geantwortet")),
            Some(_) => {
                let produced: Vec<String> = lock(&state).produced.iter().cloned().collect();
                let prompt = llm::user_prompt(&hist, &llm::style_examples(&hist, &pool, &produced));
                let res = cfg.llm.ask(&prompt, &cancel);
                if !cancel.load(Ordering::Relaxed) {
                    match &res {
                        Ok(Some(r)) if cfg.send => {
                            let answer = host.call("space.send", &serde_json::json!({ "space": space, "text": r }).to_string()).unwrap_or_default();
                            match serde_json::from_str::<serde_json::Value>(&answer).ok().and_then(|v| v["error"].as_str().map(String::from)) {
                                Some(e) => { host.log(&format!("Senden fehlgeschlagen: {e}")); host.toast(&format!("Senden fehlgeschlagen: {e}")) }
                                None => host.log(&format!("an {author} gesendet")),
                            }
                        }
                        Ok(Some(r)) => { host.log(&format!("Vorschlag für {author} erstellt")); host.toast(&format!("Vorschlag für {author}: {r}")) }
                        Ok(None) => tell(&format!("Keine Antwort nötig ({author}): das Modell hat {} geantwortet.", llm::NO_REPLY)),
                        // Errors always show, also when sending automatically.
                        Err(e) => { host.log(&format!("Anbieter-Fehler: {e}")); host.toast(&format!("Anbieter-Fehler: {e}")) }
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
    const NAME: &'static str = "Auto-Antwort";
    const AUTHOR: &'static str = "Webox examples";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const DESCRIPTION: &'static str = env!("CARGO_PKG_DESCRIPTION");
    const EVENTS: &'static str = "message.received";

    fn start(&mut self, host: Host) {
        self.cancel.store(false, Ordering::Relaxed);
        if !["message.received", "space.history", "space.send"].iter().all(|n| host.has(n)) {
            host.toast("Diese Webox-Version meldet Plugins keine eingehenden Nachrichten (message.received, space.history oder space.send fehlt). Bitte Webox aktualisieren.");
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
            .section("Auto-Antwort", "Beantwortet eingehende Nachrichten in Ihrem eigenen Stil: Ein Sprachmodell liest den Verlauf und Ihre früheren Nachrichten und schreibt die Antwort, die Sie schreiben würden, oder keine.")
            .toggle("on", "Auto-Antwort einschalten", "", false)
            .choice("mode", "Modus", "Ein Vorschlag erscheint als Hinweis; „Automatisch senden“ schickt die Antwort selbst.", "suggest",
                &[("suggest", "Vorschlag als Toast"), ("send", "Automatisch senden")])
            .choice("scope", "Wann antworten", "", "direct", &[("direct", "Nur in 1:1-Chats"), ("mentions", "1:1-Chats und Gruppen, wenn ich @-erwähnt werde")])
            .choice("people_mode", "Personen", "", "all", &[("all", "Alle"), ("allow", "Nur Personen aus der Liste"), ("deny", "Alle außer Personen aus der Liste")])
            .multiline("people", "Liste", "Ein Anzeigename pro Zeile, wie er in Webox steht.", "")
            .number("delay", "Verzögerung (s)", "So lange wird gewartet; antworten Sie in der Zeit selbst, entfällt die Auto-Antwort.", 20.0, 0.0, 600.0, 1.0)
            .number("cooldown", "Höchstens eine Auto-Antwort pro Bereich alle (min)", "Verhindert Endlosschleifen mit anderen Bots.", 10.0, 1.0, 1440.0, 1.0)
            .section("Sprachmodell", "Lokale CLIs laufen ohne Werkzeuge und mit Ihrem dortigen Login.")
            .choice("provider", "Anbieter", "", "claude", &[("claude", "Claude Code (claude)"), ("codex", "Codex (codex)"), ("cursor", "Cursor (cursor-agent)"), ("custom", "Eigener Endpunkt (OpenAI-kompatibel)")])
            .text("cli", "Programmpfad", "Leer = claude / codex / cursor-agent / curl aus dem PATH.", "")
            .text("model", "Modell", "Leer = Standard des CLIs. Beim eigenen Endpunkt erforderlich.", "")
            .text("endpoint", "Endpunkt-URL", "Nur für den eigenen Endpunkt, z. B. https://api.openai.com/v1 (…/chat/completions wird ergänzt).", "")
            .password("token", "API-Token", "Nur für den eigenen Endpunkt. Wird im Schlüsselbund gespeichert.")
            .button("test", "Verbindung testen", "Fragt das Sprachmodell einmal und zeigt das Ergebnis."))
    }
    fn settings_changed(&mut self, v: &Values) { self.cfg = Cfg::from(v); }
    fn settings_action(&mut self, id: &str) -> Option<String> {
        if id != "test" { return None; }
        let Some(host) = self.host else { return Some("Bitte das Plugin zuerst einschalten.".into()) };
        let (llm, cancel) = (self.cfg.llm.clone(), self.cancel.clone());
        self.spawn(move || {
            let t0 = Instant::now();
            let hist = [Msg { author: "Test".into(), mine: false, text: "Hallo! Kurzer Verbindungstest, alles gut bei dir?".into() }];
            let res = llm.ask(&llm::user_prompt(&hist, &[]), &cancel);
            if cancel.load(Ordering::Relaxed) { return; }
            let s = t0.elapsed().as_secs_f32();
            host.toast(&match res {
                Ok(Some(r)) => format!("Verbindung ok ({s:.1} s): {r}"),
                Ok(None) => format!("Verbindung ok ({s:.1} s), das Modell wollte nicht antworten"),
                Err(e) => format!("Verbindung fehlgeschlagen: {e}"),
            });
        });
        Some("Test läuft, das Ergebnis erscheint als Hinweis.".into())
    }

    fn on_event(&mut self, name: &str, payload: &str) -> Option<String> {
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
                if !QUIET.contains(&why) { host.log(&format!("übersprungen ({}): {why}", m.author)); }
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
mod tests;
