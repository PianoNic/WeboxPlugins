//! Message Logger: Equicord's MessageLogger for Webox. Deleted messages stay visible, marked as deleted; edited
//! messages show their earlier versions, optionally as a coloured diff; "(edited)" opens the edit history dialog.
//!
//! Contract 1.6 events and methods (docs/plugins.md in the Webox repository):
//! - `message.deleted` / `message.edited`: the answer keeps the deleted message (`{"keep", "style", "collapsed",
//!   "attachments"}`) or attaches the earlier texts (`{"history", "current", "inline"}`, plain strings or diff spans).
//! - `message.keep` gives the whole log back after a settings change, a menu action or a restart (`replace`).
//! - `message.menu` / `message.menu_action`: "Toggle Deleted Highlight", "Enable Diff View" / "Disable Diff View" and
//!   "Clear Message History", as in Equicord.
//! - `space.menu` / `space.menu_action`: "Clear Message Log" on a space with something logged, as in Equicord.
//! - `message.edit_tag_clicked` + `ui.dialog`: the "Message Edit History" dialog (Equicord's HistoryModal), one tab per version
//!   and the switch "Show Diff".
//!
//! Everything stays on this computer: in memory, and only with "Keep the log after a restart" also in the plugin's data store
//! (`host.set`, Webox's plugin-data.json). Nothing is ever sent.
use collabhost_plugin_api::{export_plugin, Host, Plugin, Schema, Values};
use serde_json::{json, Value};
use std::collections::HashSet;

mod diff;
mod i18n;
use diff::{diff, Part};

/// At most this many deleted messages and edited messages are kept (the oldest go first).
const MAX: usize = 500;
/// At most this many earlier texts per message.
const MAX_VERSIONS: usize = 20;
/// The key in the plugin's data store.
const STORE: &str = "log";

/// The settings, named after Equicord's (`deleteStyle`, `logDeletes`, ...), plus "persist".
#[derive(Clone, Debug, PartialEq)]
struct Cfg {
    /// deleteStyle: "text" (Red text) or "overlay" (Red overlay).
    style: String,
    log_deletes: bool,
    /// collapseDeleted: folded like blocked messages ("N deleted messages"), a click unfolds them.
    collapse: bool,
    log_edits: bool,
    /// inlineEdits: the earlier texts in the message; off: only "(edited)", whose dialog shows them.
    inline: bool,
    /// logDeletedAttachments: a kept deleted message keeps its pictures, files and link cards.
    attachments: bool,
    ignore_bots: bool, ignore_self: bool, ignore_self_edits: bool,
    /// ignoreUsers / ignoreChannels / ignoreGuilds (teams): names, e-mail addresses or ids, lower case.
    users: Vec<String>, spaces: Vec<String>, teams: Vec<String>,
    /// showEditDiffs / separatedDiffs.
    diffs: bool, separated: bool,
    persist: bool,
}
impl Default for Cfg {
    // The same defaults the schema declares (Equicord's).
    fn default() -> Self {
        Cfg { style: "text".into(), log_deletes: true, collapse: false, log_edits: true, inline: true, attachments: true, ignore_bots: true, ignore_self: false,
            ignore_self_edits: false, users: vec![], spaces: vec![], teams: vec![], diffs: false, separated: false, persist: false }
    }
}
/// A list setting: one entry per line or comma, trimmed, lower case.
fn list(t: Option<&str>) -> Vec<String> { t.unwrap_or("").split([',', '\n']).map(|l| l.trim().to_lowercase()).filter(|l| !l.is_empty()).collect() }
impl Cfg {
    fn from(v: &Values) -> Cfg {
        let d = Cfg::default();
        let b = |k: &str, d: bool| v.bool(k).unwrap_or(d);
        let diffs = b("showEditDiffs", d.diffs);
        Cfg {
            style: if v.text("deleteStyle") == Some("overlay") { "overlay".into() } else { "text".into() },
            log_deletes: b("logDeletes", d.log_deletes), collapse: b("collapseDeleted", d.collapse), log_edits: b("logEdits", d.log_edits),
            inline: b("inlineEdits", d.inline), attachments: b("logDeletedAttachments", d.attachments), ignore_bots: b("ignoreBots", d.ignore_bots),
            ignore_self: b("ignoreSelf", d.ignore_self), ignore_self_edits: b("ignoreSelfEdits", d.ignore_self_edits),
            users: list(v.text("ignoreUsers")), spaces: list(v.text("ignoreChannels")), teams: list(v.text("ignoreGuilds")),
            // As in Equicord: separated diffs only with diffs.
            diffs, separated: diffs && b("separatedDiffs", d.separated),
            persist: b("persist", d.persist),
        }
    }

    /// Equicord's `shouldIgnore`: whether a message (a `message.deleted` / `message.edited` payload or a logged record) is left out.
    fn ignores(&self, r: &Value, edit: bool) -> bool {
        let low = |k: &str| s(r, k).to_lowercase();
        let hit = |l: &[String], keys: &[&str]| keys.iter().map(|k| low(k)).any(|x| !x.is_empty() && l.contains(&x));
        (self.ignore_bots && is_bot(s(r, "email")))
            || (self.ignore_self && r["mine"] == true)
            || (self.ignore_self_edits && edit && r["mine"] == true)
            || hit(&self.users, &["author", "email"])
            || hit(&self.spaces, &["space", "space_name"])
            || hit(&self.teams, &["team", "team_name"])
            || if edit { !self.log_edits } else { !self.log_deletes }
    }
}

/// Webex bots have addresses of these domains (the message events carry no bot flag).
fn is_bot(email: &str) -> bool {
    let e = email.to_lowercase();
    ["@webex.bot", "@sparkbot.io"].iter().any(|d| e.ends_with(d))
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str { v[k].as_str().unwrap_or("") }

/// What was logged, oldest first: `message.deleted` payloads, and per edited message a record `{"id", "space", "space_name",
/// "team", "team_name", "author", "email", "mine", "published_ms", "old_edited", "history": [{"text", "ms"}], "current",
/// "current_ms"}`: the earlier texts with the time each became the text (0 = not known), and the current one.
#[derive(Clone, Debug, Default, PartialEq)]
struct Log { deleted: Vec<Value>, edited: Vec<Value> }
impl Log {
    fn parse(t: &str) -> Log {
        let v: Value = serde_json::from_str(t).unwrap_or(Value::Null);
        let list = |k: &str| v[k].as_array().cloned().unwrap_or_default().into_iter().filter(|r| !s(r, "id").is_empty()).collect::<Vec<Value>>();
        // 1.1.0 kept the earlier texts as plain strings.
        let edited = list("edited").into_iter().map(|mut r| {
            let h: Vec<Value> = r["history"].as_array().cloned().unwrap_or_default().into_iter()
                .map(|x| if x.is_string() { json!({ "text": x, "ms": 0 }) } else { x }).collect();
            r["history"] = json!(h);
            r
        }).collect();
        Log { deleted: list("deleted"), edited }
    }
    fn to_json(&self) -> String { json!({ "deleted": self.deleted, "edited": self.edited }).to_string() }

    fn is_deleted(&self, id: &str) -> bool { self.deleted.iter().any(|r| s(r, "id") == id) }
    fn edit(&self, id: &str) -> Option<&Value> { self.edited.iter().find(|r| s(r, "id") == id) }
    /// Equicord's `doesMessageHaveHistory`.
    fn has_history(&self, id: &str) -> bool { self.is_deleted(id) || self.edit(id).is_some() }
    fn has_space(&self, space: &str) -> bool { self.deleted.iter().chain(&self.edited).any(|r| s(r, "space") == space) }

    /// Equicord's `clearMessageHistory`: a deleted message is forgotten (Webox then removes it as any deleted message), an
    /// edited one loses its earlier texts.
    fn clear_message(&mut self, id: &str) { self.deleted.retain(|r| s(r, "id") != id); self.edited.retain(|r| s(r, "id") != id); }
    /// "Clear Message Log": everything logged in `space`.
    fn clear_space(&mut self, space: &str) { self.deleted.retain(|r| s(r, "space") != space); self.edited.retain(|r| s(r, "space") != space); }

    fn add_deleted(&mut self, rec: Value) {
        self.deleted.retain(|r| s(r, "id") != s(&rec, "id"));
        self.deleted.push(rec);
        if self.deleted.len() > MAX { self.deleted.remove(0); }
    }

    /// Adds an edit (`message.edited` payload) noticed at `now` (Unix ms); the record afterwards.
    fn add_edit(&mut self, ev: &Value, now: i64) -> &Value {
        let id = s(ev, "id");
        let i = self.edited.iter().position(|r| s(r, "id") == id);
        let mut rec = i.map(|i| self.edited.remove(i)).unwrap_or_else(|| json!({ "id": id, "history": [], "old_edited": ev["old_edited"], "current_ms": 0 }));
        for k in ["space", "space_name", "team", "team_name", "author", "email", "mine", "published_ms"] { if !ev[k].is_null() { rec[k] = ev[k].clone(); } }
        let mut h: Vec<Value> = rec["history"].as_array().cloned().unwrap_or_default();
        let old = s(ev, "old_text");
        if h.last().map(|x| s(x, "text")) != Some(old) {
            // The old text became the text at the previous edit Webox saw, or (the first one) when it was written.
            let ms = if h.is_empty() { if ev["old_edited"] == true { 0 } else { ev["published_ms"].as_i64().unwrap_or(0) } } else { rec["current_ms"].as_i64().unwrap_or(0) };
            h.push(json!({ "text": old, "ms": ms }));
        }
        // A text edited back to an earlier version is no longer an earlier version.
        h.retain(|x| s(x, "text") != s(ev, "new_text"));
        if h.len() > MAX_VERSIONS { h.drain(..h.len() - MAX_VERSIONS); }
        rec["history"] = json!(h);
        rec["current"] = ev["new_text"].clone();
        rec["current_ms"] = json!(now);
        self.edited.push(rec);
        if self.edited.len() > MAX { self.edited.remove(0); }
        self.edited.last().unwrap()
    }
}

/// Spans as Webox draws them: `{"text", "kind"}`, kind "added" / "removed" for the parts `mark` lets through, else plain.
fn spans(parts: &[Part], mark: fn(&Part) -> bool) -> Value {
    json!(parts.iter().map(|p| if !matches!(p, Part::Same(_)) && mark(p) { json!({ "text": p.text(), "kind": p.kind() }) } else { json!({ "text": p.text() }) }).collect::<Vec<_>>())
}
fn any_change(_: &Part) -> bool { true }
fn only_removed(p: &Part) -> bool { matches!(p, Part::Removed(_)) }
fn only_added(p: &Part) -> bool { matches!(p, Part::Added(_)) }
/// Equicord's `buildViewSegments`: the old side (without the added parts) or the new side (without the removed ones).
fn side(parts: &[Part], old: bool) -> Vec<Part> {
    parts.iter().filter(|p| !matches!((p, old), (Part::Added(_), true) | (Part::Removed(_), false))).cloned().collect()
}

/// Edit records' texts: the earlier ones (oldest first) and the current one.
fn texts(rec: &Value) -> (Vec<String>, String) {
    (rec["history"].as_array().into_iter().flatten().map(|x| s(x, "text").to_string()).collect(), s(rec, "current").to_string())
}

/// Per-message states of this session, as in Equicord (not stored).
#[derive(Clone, Debug, Default, PartialEq)]
struct Session {
    /// "Toggle Deleted Highlight": deleted messages drawn without highlight.
    plain: HashSet<String>,
    /// "Disable Diff View": messages without the diff view.
    no_diff: HashSet<String>,
}

#[derive(Default)]
pub struct MessageLogger { host: Option<Host>, cfg: Cfg, log: Log, session: Session }

impl MessageLogger {
    fn diffs_for(&self, id: &str) -> bool { self.cfg.diffs && !self.session.no_diff.contains(id) }

    /// What Webox attaches to an edited message (`message.edited` answer / `message.keep` record without the id): Equicord's
    /// `renderEdits` with `parseEditContent`.
    fn edit_view(&self, rec: &Value) -> Value {
        if !self.cfg.inline { return json!({ "history": [], "inline": false }); }
        let (h, cur) = texts(rec);
        if h.is_empty() { return json!({ "history": [] }); }
        if !self.diffs_for(s(rec, "id")) { return json!({ "history": h }); }
        if self.cfg.separated {
            // Aggregated: the original with what was removed, the current text with what was added since.
            let d = diff(&h[0], &cur);
            return json!({ "history": [spans(&side(&d, true), only_removed)], "current": spans(&side(&d, false), only_added) });
        }
        // Each earlier text against the next one.
        let lines: Vec<Value> = h.iter().enumerate().map(|(i, t)| spans(&diff(t, h.get(i + 1).unwrap_or(&cur)), any_change)).collect();
        json!({ "history": lines })
    }

    /// The `message.keep` argument for what is shown now, replacing all before.
    fn keep_args(&self) -> Value {
        let deleted: Vec<Value> = self.log.deleted.iter().filter(|r| !self.cfg.ignores(r, false)).map(|r| {
            let mut r = r.clone();
            r["style"] = json!(if self.session.plain.contains(s(&r, "id")) { "plain" } else { self.cfg.style.as_str() });
            r["collapsed"] = json!(self.cfg.collapse);
            r["attachments"] = json!(self.cfg.attachments);
            r
        }).collect();
        let edited: Vec<Value> = self.log.edited.iter().filter(|r| !self.cfg.ignores(r, true)).map(|r| {
            let mut e = self.edit_view(r);
            e["id"] = r["id"].clone();
            e
        }).collect();
        json!({ "replace": true, "deleted": deleted, "edited": edited })
    }

    /// The message menu's entries (`message.menu` payload), Equicord's `patchMessageContextMenu`.
    fn menu(&self, ev: &Value) -> Vec<Value> {
        let id = s(ev, "id");
        let mut v = vec![];
        if id.is_empty() || !self.log.has_history(id) { return v; }
        if ev["deleted"] == true && self.log.is_deleted(id) {
            v.push(json!({ "id": "toggle-deleted", "label": "Toggle Deleted Highlight", "icon": "show-regular" }));
        }
        if self.cfg.diffs && self.log.edit(id).is_some() {
            let on = self.diffs_for(id);
            v.push(json!({ "id": "toggle-diff", "label": if on { "Disable Diff View" } else { "Enable Diff View" }, "danger": true }));
        }
        v.push(json!({ "id": "clear-history", "label": "Clear Message History", "icon": "recents-regular", "danger": true }));
        v
    }

    /// A picked message-menu entry; whether what is shown changed.
    fn menu_action(&mut self, ev: &Value) -> bool {
        let id = s(ev, "id").to_string();
        let flip = |set: &mut HashSet<String>| if !set.remove(&id) { set.insert(id.clone()); };
        match s(ev, "item") {
            "toggle-deleted" if self.log.is_deleted(&id) => flip(&mut self.session.plain),
            "toggle-diff" if self.log.edit(&id).is_some() => flip(&mut self.session.no_diff),
            "clear-history" if self.log.has_history(&id) => { self.log.clear_message(&id); self.session.plain.remove(&id); self.save(); }
            _ => return false,
        }
        true
    }

    /// The "Message Edit History" dialog for a click on "(edited)" (`message.edit_tag_clicked` payload): Equicord's
    /// HistoryModal. One tab per logged version (with the time it became the text) and the current one, which is shown first;
    /// before them a greyed tab when the original text was never seen. "Show Diff" diffs each against the newest.
    fn history_dialog(&self, ev: &Value) -> Value {
        let id = s(ev, "id");
        let rec = self.log.edit(id).filter(|r| !self.cfg.ignores(r, true));
        let (h, ms): (Vec<String>, Vec<i64>) = rec.map(|r| r["history"].as_array().into_iter().flatten().map(|x| (s(x, "text").to_string(), x["ms"].as_i64().unwrap_or(0))).unzip()).unwrap_or_default();
        let cur = rec.map_or_else(|| s(ev, "text").to_string(), |r| s(r, "current").to_string());
        let cur_ms = rec.and_then(|r| r["current_ms"].as_i64()).unwrap_or(0);
        let mut tabs = vec![];
        // The first version Webox saw was already an edit (or nothing was logged): the original is not known.
        let unseen = rec.is_none_or(|r| r["old_edited"] == true);
        let published = ev["published_ms"].as_i64().or_else(|| rec.and_then(|r| r["published_ms"].as_i64())).unwrap_or(0);
        if unseen && published > 0 {
            tabs.push(json!({ "time_ms": published, "text": "", "disabled": true, "hint": "This edit state was not logged so it can't be displayed." }));
        }
        // As Equicord: no diff for a message whose diff view was switched off from its menu.
        let can_diff = !self.session.no_diff.contains(id);
        let label = |ms: i64| if ms > 0 { json!({ "time_ms": ms }) } else { json!({ "label": "Earlier version" }) };
        for (t, ms) in h.iter().zip(ms) {
            let mut tab = label(ms);
            tab["text"] = json!(t);
            if can_diff {
                let d = diff(t, &cur);
                tab["alt"] = if self.cfg.separated { spans(&side(&d, true), only_removed) } else { spans(&d, any_change) };
            }
            tabs.push(tab);
        }
        let mut now = if cur_ms > 0 { json!({ "time_ms": cur_ms }) } else { json!({ "label": "Current version" }) };
        now["text"] = json!(cur);
        tabs.push(now);
        let n = tabs.len() - 1;
        json!({ "title": "Message Edit History", "tabs": tabs, "selected": n, "switch": { "label": "Show Diff", "on": self.diffs_for(id) } })
    }

    /// The answer to an event (None = Webox handles it as usual), logging what it keeps.
    fn answer(&mut self, name: &str, payload: &str) -> Option<String> {
        let ev: Value = serde_json::from_str(payload).ok()?;
        match name {
            "message.menu" => { let items = self.menu(&ev); return (!items.is_empty()).then(|| json!({ "items": items }).to_string()); }
            "message.menu_action" => { if self.menu_action(&ev) { self.sync(); } return None; }
            "space.menu" => {
                return self.log.has_space(s(&ev, "space")).then(|| json!({ "items": [{ "id": "clear-log", "label": "Clear Message Log", "danger": true }] }).to_string());
            }
            "space.menu_action" => {
                if s(&ev, "item") == "clear-log" && self.log.has_space(s(&ev, "space")) { self.log.clear_space(s(&ev, "space")); self.save(); self.sync(); }
                return None;
            }
            "message.edit_tag_clicked" => {
                if let Some(h) = self.host { h.call("ui.dialog", &self.history_dialog(&ev).to_string()); }
                return None;
            }
            _ => {}
        }
        if s(&ev, "id").is_empty() { return None; }
        let a = match name {
            "message.deleted" if !self.cfg.ignores(&ev, false) => {
                self.log.add_deleted(ev);
                json!({ "keep": true, "style": self.cfg.style, "collapsed": self.cfg.collapse, "attachments": self.cfg.attachments })
            }
            "message.edited" if !self.cfg.ignores(&ev, true) => {
                let rec = self.log.add_edit(&ev, now_ms()).clone();
                self.edit_view(&rec)
            }
            _ => return None,
        };
        self.save();
        Some(a.to_string())
    }

    /// Writes the log to the data store, or empties it there when it is not to be kept.
    fn save(&self) {
        let Some(h) = self.host else { return };
        if self.cfg.persist { h.set(STORE, &self.log.to_json()) } else if h.get(STORE).is_some_and(|t| !t.is_empty()) { h.set(STORE, "") }
    }

    /// Tells Webox what to show now (after a start, a settings change or a menu action).
    fn sync(&self) {
        if let Some(h) = self.host { h.call("message.keep", &self.keep_args().to_string()); }
    }
}

fn now_ms() -> i64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64) }

impl Plugin for MessageLogger {
    const NAME: &'static str = "Message Logger";
    const AUTHOR: &'static str = "Webox examples";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const DESCRIPTION: &'static str = env!("CARGO_PKG_DESCRIPTION");
    const EVENTS: &'static str = "message.deleted message.edited message.menu message.menu_action space.menu space.menu_action message.edit_tag_clicked i18n.strings";

    fn start(&mut self, host: Host) {
        i18n::sync(host);
        if !["message.deleted", "message.edited", "message.keep"].iter().all(|n| host.has(n)) {
            host.toast("This Webox version does not tell plugins about deleted or edited messages (message.deleted, message.edited or message.keep is missing). Please update Webox.");
        }
        self.host = Some(host);
        // From the last run, if it was to be kept (merged with what this run already logged, if it was switched off and on).
        if self.cfg.persist {
            let saved = Log::parse(&host.get(STORE).unwrap_or_default());
            let now = std::mem::take(&mut self.log);
            self.log = saved;
            for r in now.deleted { self.log.add_deleted(r); }
            for r in now.edited { self.log.edited.retain(|x| s(x, "id") != s(&r, "id")); self.log.edited.push(r); }
        }
        self.sync();
    }
    fn stop(&mut self) { self.host = None; }

    fn settings(&self) -> Option<Schema> {
        Some(Schema::new()
            .section("Deleted messages", "Deleted messages stay visible, marked as deleted. Webox notices changes when it loads a space. \
                Everything stays on this computer and is never sent.")
            .choice("deleteStyle", "Delete style", "How deleted messages are marked.", "text", &[("text", "Red text"), ("overlay", "Red overlay")])
            .toggle("logDeletes", "Log deleted messages", "", true)
            .toggle("collapseDeleted", "Collapse deleted messages", "Like blocked messages: \"N deleted messages\", a click expands them.", false)
            .toggle("logDeletedAttachments", "Log deleted attachments", "Pictures, files and link previews of deleted messages stay (grey, in colour under the mouse pointer).", true)
            .section("Edited messages", "")
            .toggle("logEdits", "Log edits", "", true)
            .toggle("inlineEdits", "Show edits inline", "Off: only \"(edited)\"; a click on it shows the edit history.", true)
            .toggle("showEditDiffs", "Show edit diffs", "Added text in green, removed text in red.", false)
            .toggle("separatedDiffs", "Show separated diffs", "Only with \"Show edit diffs\": the first version with what was removed, the current text with what was added.", false)
            .section("Ignore", "Lists: one entry per line or separated by commas, not case-sensitive.")
            .toggle("ignoreBots", "Ignore bots", "Messages from Webex bots (addresses ending in @webex.bot).", true)
            .toggle("ignoreSelf", "Ignore your own messages", "", false)
            .toggle("ignoreSelfEdits", "Ignore your own edits", "Has no effect while \"Ignore your own messages\" is on.", false)
            .multiline("ignoreUsers", "Ignored people", "Names or e-mail addresses.", "")
            .multiline("ignoreChannels", "Ignored spaces", "Space names as Webox shows them, or space ids.", "")
            .multiline("ignoreGuilds", "Ignored teams", "Team names or team ids: all spaces of these teams.", "")
            .section("Storage", "The log is stored only on this computer and never sent. Without the option below it is kept in memory only.")
            .toggle("persist", "Keep the log after a restart", "Stores the log in Webox's plugin data on this computer.", false)
            .button("clear", "Clear Message Log", "Forgets all deleted messages and earlier versions."))
    }
    fn settings_changed(&mut self, v: &Values) {
        self.cfg = Cfg::from(v);
        self.save();
        self.sync();
    }
    fn settings_action(&mut self, id: &str) -> Option<String> {
        if id != "clear" { return None; }
        self.log = Log::default();
        self.session = Session::default();
        self.save();
        self.sync();
        Some(i18n::t("Message log cleared."))
    }

    fn on_event(&mut self, name: &str, payload: &str) -> Option<String> {
        if name == "i18n.strings" { return Some(i18n::strings(payload)); }
        self.answer(name, payload)
    }
}

export_plugin!(MessageLogger);

#[cfg(test)]
mod i18n_tests {
    use super::*;
    #[test]
    fn every_setting_has_a_german_text() {
        let s = MessageLogger::default().settings().unwrap().to_json();
        assert_eq!(i18n::untranslated(&s, &[MessageLogger::NAME]), Vec::<String>::new());
    }
    #[test]
    fn the_event_answers_german_and_nothing_for_other_languages() {
        let mut p = MessageLogger::default();
        assert_eq!(p.on_event("i18n.strings", r#"{"lang":"fr","fallbacks":["fr_FR","fr"]}"#).as_deref(), Some("{\"strings\":{}}"));
        assert_eq!(p.on_event("i18n.strings", r#"{"lang":"en","fallbacks":["en_US","en"]}"#).as_deref(), Some("{\"strings\":{}}"));
        let de: serde_json::Value = serde_json::from_str(&i18n::answer("de_CH")).unwrap();
        assert_eq!(de["strings"]["Clear Message Log"], "Protokoll leeren");
    }
}

#[cfg(test)]
mod tests;
