//! Webex MCP: an MCP server inside Webox, so a local AI client (Claude Code, Claude Desktop, LM Studio, ...) can read the user's
//! Webex through Webox: spaces, messages, people; and, only when the user allows it and confirms each message, send.
//!
//! Nothing listens unless the plugin is switched on and "MCP server on" is on. The server binds 127.0.0.1 only and wants a
//! bearer token, which the plugin generates on first start and keeps as a password setting (OS keyring). Every tool call is
//! logged (client, tool, space; never message text). See `server.rs` for the transport and `mcp.rs` for the tools.
//!
//! Threads: the server's accept thread and its connection threads (server.rs). Settings changes restart the server on the UI
//! thread; connection threads from before are joined at the latest in `stop`, which leaves no thread running.
mod i18n;
mod mcp;
mod rand;
mod server;

use collabhost_plugin_api::{export_plugin, Host, Plugin, Schema, Values};
use i18n::t;
use mcp::{Api, Policy};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub const DEFAULT_PORT: u16 = 39847;
/// Lines the "Show recent calls" button shows.
const RECENT: usize = 20;
/// Host methods the tools use.
const NEEDS: &[&str] = &["spaces.list", "space.history", "messages.search", "people.search", "ui.confirm", "space.send", "settings.set", "clipboard.write"];

#[derive(Clone, Debug, PartialEq)]
struct Cfg { active: bool, port: u16, token: String, policy: Policy }
impl Default for Cfg { fn default() -> Self { Cfg { active: false, port: DEFAULT_PORT, token: String::new(), policy: Policy::default() } } }
impl Cfg {
    fn from(v: &Values) -> Cfg {
        Cfg {
            active: v.bool("active").unwrap_or(false),
            port: v.number("port").map_or(DEFAULT_PORT, |p| p as u16),
            token: v.text("token").unwrap_or("").trim().to_string(),
            policy: Policy {
                send: v.bool("send").unwrap_or(false),
                allow: v.text("spaces").unwrap_or("").lines().map(|l| l.trim().to_lowercase()).filter(|l| !l.is_empty()).collect(),
            },
        }
    }
    fn url(&self) -> String { format!("http://127.0.0.1:{}/mcp", self.port) }
}

/// The ready-to-paste client configuration ("Copy connection").
fn snippet(c: &Cfg) -> String {
    let (url, auth) = (c.url(), format!("Bearer {}", c.token));
    let cfg = json!({ "mcpServers": { "webex": { "type": "http", "url": url, "headers": { "Authorization": auth } } } });
    format!("# Claude Code\nclaude mcp add --transport http webex {url} --header \"Authorization: {auth}\"\n\n# mcp.json (LM Studio, Cursor, ...)\n{}\n",
        serde_json::to_string_pretty(&cfg).unwrap_or_default())
}

/// `Host` as the tools see it; log lines are also kept for the "Show recent calls" button.
struct Webox { host: Host, recent: Arc<Mutex<VecDeque<String>>> }
// Webox's host methods are safe to call from any thread (docs/plugins.md); `Host` stays valid until `stop` returns, and every
// thread using it is joined before that.
unsafe impl Sync for Webox {}
impl Api for Webox {
    fn call(&self, method: &str, args: Value) -> Value {
        self.host.call(method, &args.to_string()).and_then(|r| serde_json::from_str(&r).ok())
            .unwrap_or_else(|| json!({ "error": format!("Webox does not know {method}; please update Webox") }))
    }
    fn log(&self, text: &str) {
        self.host.log(text);
        push_recent(&self.recent, text);
    }
}
fn push_recent(r: &Mutex<VecDeque<String>>, text: &str) {
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) % 86400;
    let mut r = r.lock().unwrap_or_else(|e| e.into_inner());
    r.push_back(format!("{:02}:{:02}:{:02} UTC  {text}", s / 3600, s / 60 % 60, s % 60));
    while r.len() > RECENT { r.pop_front(); }
}

fn new_session_id() -> String { rand::hex(16).unwrap_or_default() }

#[derive(Default)]
pub struct WebexMcp {
    host: Option<Host>, cfg: Cfg,
    state: server::Shared,
    recent: Arc<Mutex<VecDeque<String>>>,
    server: Option<server::Server>,
    /// Connection threads of servers stopped by a settings change; joined in `stop` at the latest.
    leftover: Vec<JoinHandle<()>>,
}

impl WebexMcp {
    /// Brings the server in line with the settings: running on the configured port exactly when switched on and a token exists.
    fn apply(&mut self) {
        {
            let mut s = server::lock(&self.state);
            if s.token != self.cfg.token { s.sessions.clear(); }
            s.token = self.cfg.token.clone();
            s.policy = self.cfg.policy.clone();
        }
        self.leftover.retain(|h| !h.is_finished());
        let Some(host) = self.host else { return };
        let want = self.cfg.active && !self.cfg.token.is_empty();
        if want && self.server.as_ref().is_some_and(|s| s.port == self.cfg.port) { return; }
        if let Some(s) = self.server.take() { self.leftover.extend(s.stop()); host.log(&t("MCP server stopped")); }
        if !want { return; }
        let api: Arc<dyn Api> = Arc::new(Webox { host, recent: self.recent.clone() });
        match server::start(self.cfg.port, api, self.state.clone(), new_session_id) {
            Ok(s) => { host.log(&t("MCP server running: {url}").replace("{url}", &self.cfg.url())); self.server = Some(s); }
            Err(e) => { let m = t("MCP server could not open port {port}: {e}").replace("{port}", &self.cfg.port.to_string()).replace("{e}", &e.to_string()); host.log(&m); host.toast(&m); }
        }
    }

    /// Makes a new token and saves it as the password setting (keyring); in effect at once, saved on the next frame.
    fn new_token(&mut self) -> Result<(), String> {
        let host = self.host.ok_or_else(|| t("Please turn the plugin on first."))?;
        let token = rand::hex(32).ok_or_else(|| t("The system returned no random numbers."))?;
        host.call("settings.set", &json!({ "id": "token", "value": token }).to_string());
        self.cfg.token = token;
        self.apply();
        Ok(())
    }
}

impl Plugin for WebexMcp {
    const NAME: &'static str = "Webex MCP";
    const AUTHOR: &'static str = "Webox examples";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const DESCRIPTION: &'static str = env!("CARGO_PKG_DESCRIPTION");
    // Subscribing is what makes Webox keep the copy of spaces, people and messages that the read methods use; the event itself
    // is ignored.
    const EVENTS: &'static str = "message.received i18n.strings";

    fn start(&mut self, host: Host) {
        self.host = Some(host);
        i18n::sync(host);
        let missing: Vec<&str> = NEEDS.iter().copied().filter(|n| !host.has(n)).collect();
        if !missing.is_empty() {
            host.toast(&t("This Webox version cannot run the MCP server (missing: {missing}). Please update Webox.").replace("{missing}", &missing.join(", ")));
            return;
        }
        if self.cfg.token.is_empty() {
            if let Err(e) = self.new_token() { host.toast(&e); }
        }
        self.apply();
    }
    fn stop(&mut self) {
        if let Some(s) = self.server.take() { self.leftover.extend(s.stop()); }
        // Webox answers our open confirmations with "no" before calling stop, so these end quickly.
        for h in self.leftover.drain(..) { let _ = h.join(); }
        self.host = None;
        server::lock(&self.state).sessions.clear();
    }

    fn settings(&self) -> Option<Schema> {
        Some(Schema::new()
            .section("MCP server", "Lets a local AI (Claude Code, Claude Desktop, LM Studio, …) read your Webex spaces, messages and people through the Model Context Protocol. The server listens on 127.0.0.1 only and requires the token.")
            .toggle("active", "MCP server on", "", false)
            .number("port", "Port", "Address: http://127.0.0.1:<port>/mcp", DEFAULT_PORT as f64, 1024.0, 65535.0, 1.0)
            .password("token", "Token", "Created on the first start and stored in the OS keyring. Clients send it as \"Authorization: Bearer <token>\".")
            .button("show_token", "Show token", "")
            .button("new_token", "Generate new token", "Connected clients then need the new token.")
            .button("copy", "Copy connection", "Puts the Claude Code command and an mcp.json with address and token on the clipboard.")
            .section("Permissions", "The AI reads only what Webox has loaded. Every call is logged (client, tool, space; never message text).")
            .toggle("send", "Allow the AI to send messages", "You confirm every message in Webox with \"Send\".", false)
            .multiline("spaces", "Only these spaces", "One space name (or id) per line. Empty = all spaces are visible to the AI.", "")
            .button("calls", "Show recent calls", ""))
    }
    fn settings_changed(&mut self, v: &Values) {
        let token = self.cfg.token.clone();
        self.cfg = Cfg::from(v);
        // Cleared on the page: a running plugin makes a new one (the server never runs without).
        if self.cfg.token.is_empty() && !token.is_empty() && self.host.is_some() { let _ = self.new_token(); }
        self.apply();
    }
    fn settings_action(&mut self, id: &str) -> Option<String> {
        Some(match id {
            "show_token" if self.cfg.token.is_empty() => t("No token yet: please turn the plugin on."),
            "show_token" => format!("Token: {}", self.cfg.token),
            "new_token" => match self.new_token() { Ok(()) => t("New token generated. Connected clients need the new token."), Err(e) => e },
            "copy" => {
                let Some(host) = self.host.filter(|_| !self.cfg.token.is_empty()) else { return Some(t("Please turn the plugin on first.")) };
                host.call("clipboard.write", &json!({ "text": snippet(&self.cfg) }).to_string());
                t("Connection copied ({url})").replace("{url}", &self.cfg.url()) + &if self.cfg.active { String::new() } else { t(". Note: \"MCP server on\" is off.") }
            }
            "calls" => {
                let r = self.recent.lock().unwrap_or_else(|e| e.into_inner());
                if r.is_empty() { t("No calls yet.") } else { r.iter().rev().take(8).cloned().collect::<Vec<_>>().join("\n") }
            }
            _ => return None,
        })
    }
    fn on_event(&mut self, name: &str, payload: &str) -> Option<String> { (name == "i18n.strings").then(|| i18n::strings(payload)) }
}

export_plugin!(WebexMcp);

#[cfg(test)]
mod i18n_tests {
    use super::*;
    #[test]
    fn every_setting_has_a_german_text() {
        let s = WebexMcp::default().settings().unwrap().to_json();
        assert_eq!(i18n::untranslated(&s, &[]), Vec::<String>::new());
    }
    #[test]
    fn the_event_answers_german_and_nothing_for_other_languages() {
        let mut p = WebexMcp::default();
        assert_eq!(p.on_event("i18n.strings", r#"{"lang":"fr","fallbacks":["fr_FR","fr"]}"#).as_deref(), Some("{\"strings\":{}}"));
        assert_eq!(p.on_event("i18n.strings", r#"{"lang":"en","fallbacks":["en_US","en"]}"#).as_deref(), Some("{\"strings\":{}}"));
        let de: serde_json::Value = serde_json::from_str(&i18n::answer("de_CH")).unwrap();
        assert_eq!(de["strings"]["Show token"], "Token anzeigen");
    }
}

#[cfg(test)]
mod tests;
