//! Nachrichtenprotokoll: Equicord's MessageLogger for Webox. Deleted messages stay visible, marked as deleted; edited
//! messages show their earlier versions, optionally as a coloured diff; "(bearbeitet)" opens the edit history dialog.
//!
//! Contract 1.6 events and methods (docs/plugins.md in the Webox repository):
//! - `message.deleted` / `message.edited`: the answer keeps the deleted message (`{"keep", "style", "collapsed",
//!   "attachments"}`) or attaches the earlier texts (`{"history", "current", "inline"}`, plain strings or diff spans).
//! - `message.keep` gives the whole log back after a settings change, a menu action or a restart (`replace`).
//! - `message.menu` / `message.menu_action`: "Gelöscht-Markierung umschalten", "Änderungsvergleich aktivieren / deaktivieren",
//!   "Nachrichtenverlauf löschen" (Equicord's Toggle Deleted Highlight, Enable / Disable Diff View, Clear Message History).
//! - `space.menu` / `space.menu_action`: "Protokoll leeren" on a space with something logged (Equicord's Clear Message Log).
//! - `message.edit_tag_clicked` + `ui.dialog`: the "Bearbeitungsverlauf" dialog (Equicord's HistoryModal), one tab per version
//!   and the switch "Unterschiede anzeigen".
//!
//! Everything stays on this computer: in memory, and only with "Nach Neustart behalten" also in the plugin's data store
//! (`host.set`, Webox's plugin-data.json). Nothing is ever sent.
use collabhost_plugin_api::{export_plugin, Host, Plugin, Schema, Values};
use serde_json::{json, Value};
use std::collections::HashSet;

mod diff;
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
    /// deleteStyle: "text" (Roter Text) or "overlay" (Rote Überlagerung).
    style: String,
    log_deletes: bool,
    /// collapseDeleted: folded like blocked messages ("N gelöschte Nachrichten"), a click unfolds them.
    collapse: bool,
    log_edits: bool,
    /// inlineEdits: the earlier texts in the message; off: only "(bearbeitet)", whose dialog shows them.
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
    /// "Protokoll leeren": everything logged in `space`.
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
    /// "Gelöscht-Markierung umschalten": deleted messages drawn without highlight.
    plain: HashSet<String>,
    /// "Änderungsvergleich deaktivieren": messages without the diff view.
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
            v.push(json!({ "id": "toggle-deleted", "label": "Gelöscht-Markierung umschalten", "icon": "show-regular" }));
        }
        if self.cfg.diffs && self.log.edit(id).is_some() {
            let on = self.diffs_for(id);
            v.push(json!({ "id": "toggle-diff", "label": if on { "Änderungsvergleich deaktivieren" } else { "Änderungsvergleich aktivieren" }, "danger": true }));
        }
        v.push(json!({ "id": "clear-history", "label": "Nachrichtenverlauf löschen", "icon": "recents-regular", "danger": true }));
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

    /// The "Bearbeitungsverlauf" dialog for a click on "(bearbeitet)" (`message.edit_tag_clicked` payload): Equicord's
    /// HistoryModal. One tab per logged version (with the time it became the text) and the current one, which is shown first;
    /// before them a greyed tab when the original text was never seen. "Unterschiede anzeigen" diffs each against the newest.
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
            tabs.push(json!({ "time_ms": published, "text": "", "disabled": true, "hint": "Dieser Bearbeitungsstand wurde nicht protokolliert und kann nicht angezeigt werden." }));
        }
        // As Equicord: no diff for a message whose diff view was switched off from its menu.
        let can_diff = !self.session.no_diff.contains(id);
        let label = |ms: i64| if ms > 0 { json!({ "time_ms": ms }) } else { json!({ "label": "Frühere Fassung" }) };
        for (t, ms) in h.iter().zip(ms) {
            let mut tab = label(ms);
            tab["text"] = json!(t);
            if can_diff {
                let d = diff(t, &cur);
                tab["alt"] = if self.cfg.separated { spans(&side(&d, true), only_removed) } else { spans(&d, any_change) };
            }
            tabs.push(tab);
        }
        let mut now = if cur_ms > 0 { json!({ "time_ms": cur_ms }) } else { json!({ "label": "Aktuelle Fassung" }) };
        now["text"] = json!(cur);
        tabs.push(now);
        let n = tabs.len() - 1;
        json!({ "title": "Bearbeitungsverlauf", "tabs": tabs, "selected": n, "switch": { "label": "Unterschiede anzeigen", "on": self.diffs_for(id) } })
    }

    /// The answer to an event (None = Webox handles it as usual), logging what it keeps.
    fn answer(&mut self, name: &str, payload: &str) -> Option<String> {
        let ev: Value = serde_json::from_str(payload).ok()?;
        match name {
            "message.menu" => { let items = self.menu(&ev); return (!items.is_empty()).then(|| json!({ "items": items }).to_string()); }
            "message.menu_action" => { if self.menu_action(&ev) { self.sync(); } return None; }
            "space.menu" => {
                return self.log.has_space(s(&ev, "space")).then(|| json!({ "items": [{ "id": "clear-log", "label": "Protokoll leeren", "danger": true }] }).to_string());
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
    const NAME: &'static str = "Nachrichtenprotokoll";
    const AUTHOR: &'static str = "Webox examples";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const DESCRIPTION: &'static str = env!("CARGO_PKG_DESCRIPTION");
    const EVENTS: &'static str = "message.deleted message.edited message.menu message.menu_action space.menu space.menu_action message.edit_tag_clicked";

    fn start(&mut self, host: Host) {
        if !["message.deleted", "message.edited", "message.keep"].iter().all(|n| host.has(n)) {
            host.toast("Diese Webox-Version meldet Plugins keine gelöschten oder bearbeiteten Nachrichten (message.deleted, message.edited oder message.keep fehlt). Bitte Webox aktualisieren.");
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
            .section("Gelöschte Nachrichten", "Gelöschte Nachrichten bleiben sichtbar und werden als gelöscht markiert. Webox merkt Änderungen beim Laden eines Bereichs. \
                Alles bleibt nur auf diesem Computer und wird nie gesendet.")
            .choice("deleteStyle", "Darstellung gelöschter Nachrichten", "", "text", &[("text", "Roter Text"), ("overlay", "Rote Überlagerung")])
            .toggle("logDeletes", "Gelöschte Nachrichten protokollieren", "", true)
            .toggle("collapseDeleted", "Gelöschte Nachrichten einklappen", "Wie blockierte Nachrichten: „N gelöschte Nachrichten“, ein Klick klappt sie auf.", false)
            .toggle("logDeletedAttachments", "Anhänge gelöschter Nachrichten behalten", "Bilder, Dateien und Linkvorschauen bleiben (grau, farbig unter dem Mauszeiger).", true)
            .section("Bearbeitete Nachrichten", "")
            .toggle("logEdits", "Bearbeitungen protokollieren", "", true)
            .toggle("inlineEdits", "Bearbeitungsverlauf in der Nachricht anzeigen", "Aus: nur „(bearbeitet)“; ein Klick darauf zeigt den Verlauf.", true)
            .toggle("showEditDiffs", "Änderungen farbig anzeigen", "Hinzugefügtes grün, Entferntes rot.", false)
            .toggle("separatedDiffs", "Hinzufügungen und Entfernungen getrennt anzeigen", "Nur mit „Änderungen farbig anzeigen“: die erste Fassung mit dem Entfernten, der aktuelle Text mit dem Hinzugefügten.", false)
            .section("Ignorieren", "Listen: ein Eintrag pro Zeile oder durch Kommas getrennt, Groß- und Kleinschreibung egal.")
            .toggle("ignoreBots", "Bots ignorieren", "Nachrichten von Webex-Bots (Adressen auf @webex.bot).", true)
            .toggle("ignoreSelf", "Eigene Nachrichten ignorieren", "", false)
            .toggle("ignoreSelfEdits", "Eigene Bearbeitungen ignorieren", "Ohne Wirkung, solange „Eigene Nachrichten ignorieren“ an ist.", false)
            .multiline("ignoreUsers", "Ignorierte Personen", "Namen oder E-Mail-Adressen.", "")
            .multiline("ignoreChannels", "Ignorierte Bereiche", "Bereichsnamen, wie sie in Webox stehen, oder Bereichs-IDs.", "")
            .multiline("ignoreGuilds", "Ignorierte Teams", "Teamnamen oder Team-IDs: alle Bereiche dieser Teams.", "")
            .section("Speicherung", "Das Protokoll wird nur auf diesem Computer gespeichert und nie gesendet. Ohne die folgende Option liegt es nur im Arbeitsspeicher.")
            .toggle("persist", "Protokoll auch nach Neustart behalten", "Speichert das Protokoll in den Plugin-Daten von Webox auf diesem Computer.", false)
            .button("clear", "Protokoll leeren", "Vergisst alle gelöschten Nachrichten und früheren Fassungen."))
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
        Some("Protokoll geleert.".into())
    }

    fn on_event(&mut self, name: &str, payload: &str) -> Option<String> { self.answer(name, payload) }
}

export_plugin!(MessageLogger);

#[cfg(test)]
mod tests;
