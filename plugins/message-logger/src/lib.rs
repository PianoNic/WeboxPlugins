//! Nachrichtenprotokoll: deleted messages stay visible, marked as deleted, and edited messages show their earlier versions
//! (like Equicord's MessageLogger). Uses the contract 1.6 events `message.deleted` / `message.edited`: the answer asks Webox
//! to keep the deleted message (`{"keep": true, "style": ...}`) or attaches the earlier texts (`{"history": [...]}`); Webox
//! draws both. The method `message.keep` gives everything back after a settings change or a restart.
//!
//! The message menu (contract 1.6 `message.menu` / `message.menu_action`, like Equicord's context-menu entries) offers
//! "Gelöschte Nachricht entfernen" on a kept deleted message, "Bearbeitungsverlauf anzeigen / ausblenden" and
//! "Bearbeitungsverlauf entfernen" on an edited one, and "Protokoll dieses Bereichs leeren" on any message of a space with
//! something logged. Each changes the log and gives Webox the new state with `message.keep` (`replace`), so a removed deleted
//! message goes as Webox deletes any message.
//!
//! Everything stays on this computer: in memory, and only with "Nach Neustart behalten" also in the plugin's data store
//! (`host.set`, Webox's plugin-data.json). Nothing is ever sent.
use collabhost_plugin_api::{export_plugin, Host, Plugin, Schema, Values};
use serde_json::{json, Value};

/// At most this many deleted messages and edited messages are kept (the oldest go first).
const MAX: usize = 500;
/// At most this many earlier texts per message.
const MAX_VERSIONS: usize = 20;
/// The key in the plugin's data store.
const STORE: &str = "log";

#[derive(Clone, Debug, PartialEq)]
struct Cfg {
    deletes: bool, edits: bool, ignore_self: bool,
    /// Own edits are not logged (Equicord's "ignoreSelfEdits"); `ignore_self` leaves out own deletes and edits.
    ignore_self_edits: bool,
    /// Earlier texts are shown in the message (Equicord's "inlineEdits"); off: logged, shown per message from the menu.
    inline_edits: bool,
    /// The plugin's entries in the message menu.
    menu: bool,
    /// "text" (red text) or "overlay" (red wash).
    style: String,
    /// "all", "allow" (only `spaces`) or "deny" (all but `spaces`); a line matches a space's name (case-insensitive) or its id.
    space_mode: String, spaces: Vec<String>,
    persist: bool,
}
impl Default for Cfg {
    // The same defaults the schema declares.
    fn default() -> Self {
        Cfg { deletes: true, edits: true, ignore_self: false, ignore_self_edits: false, inline_edits: true, menu: true, style: "text".into(), space_mode: "all".into(), spaces: vec![], persist: false }
    }
}
impl Cfg {
    fn from(v: &Values) -> Cfg {
        let d = Cfg::default();
        Cfg {
            deletes: v.bool("deletes").unwrap_or(d.deletes),
            edits: v.bool("edits").unwrap_or(d.edits),
            ignore_self: v.bool("ignore_self").unwrap_or(d.ignore_self),
            ignore_self_edits: v.bool("ignore_self_edits").unwrap_or(d.ignore_self_edits),
            inline_edits: v.bool("inline_edits").unwrap_or(d.inline_edits),
            menu: v.bool("menu").unwrap_or(d.menu),
            style: if v.text("style") == Some("overlay") { "overlay".into() } else { "text".into() },
            space_mode: v.text("space_mode").filter(|m| ["allow", "deny"].contains(m)).unwrap_or("all").to_string(),
            spaces: v.text("spaces").unwrap_or("").lines().map(|l| l.trim().to_lowercase()).filter(|l| !l.is_empty()).collect(),
            persist: v.bool("persist").unwrap_or(d.persist),
        }
    }

    /// Whether a message of this space and author is logged at all.
    fn applies(&self, rec: &Value) -> bool {
        if self.ignore_self && rec["mine"] == true { return false; }
        let (name, id) = (s(rec, "space_name").to_lowercase(), s(rec, "space").to_lowercase());
        let listed = self.spaces.iter().any(|l| *l == name || *l == id);
        match self.space_mode.as_str() { "allow" => listed, "deny" => !listed, _ => true }
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str { v[k].as_str().unwrap_or("") }

/// What was logged, oldest first: `message.deleted` payloads, and per edited message its space data and earlier texts.
/// `toggled`: edited messages whose earlier texts are shown the other way than "Bearbeitungsverlauf in der Nachricht anzeigen"
/// says (picked with "Bearbeitungsverlauf anzeigen / ausblenden").
#[derive(Clone, Debug, Default, PartialEq)]
struct Log { deleted: Vec<Value>, edited: Vec<Value>, toggled: Vec<String> }
impl Log {
    fn parse(t: &str) -> Log {
        let v: Value = serde_json::from_str(t).unwrap_or(Value::Null);
        let list = |k: &str| v[k].as_array().cloned().unwrap_or_default().into_iter().filter(|r| !s(r, "id").is_empty()).collect();
        let toggled = v["toggled"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
        Log { deleted: list("deleted"), edited: list("edited"), toggled }
    }
    fn to_json(&self) -> String { json!({ "deleted": self.deleted, "edited": self.edited, "toggled": self.toggled }).to_string() }

    fn is_deleted(&self, id: &str) -> bool { self.deleted.iter().any(|r| s(r, "id") == id) }
    fn is_edited(&self, id: &str) -> bool { self.edited.iter().any(|r| s(r, "id") == id) }
    /// Whether the earlier texts of `id` are shown now.
    fn history_shown(&self, id: &str, cfg: &Cfg) -> bool { cfg.inline_edits != self.toggled.iter().any(|t| t == id) }
    fn has_space(&self, space: &str) -> bool { self.deleted.iter().chain(&self.edited).any(|r| s(r, "space") == space) }

    /// "Gelöschte Nachricht entfernen": forgets the deleted message (Webox then removes it as usual).
    fn forget_deleted(&mut self, id: &str) { self.deleted.retain(|r| s(r, "id") != id); }
    /// "Bearbeitungsverlauf entfernen": forgets the earlier texts of `id`.
    fn forget_history(&mut self, id: &str) { self.edited.retain(|r| s(r, "id") != id); self.toggled.retain(|t| t != id); }
    /// "Bearbeitungsverlauf anzeigen / ausblenden".
    fn toggle_history(&mut self, id: &str) {
        if self.toggled.iter().any(|t| t == id) { self.toggled.retain(|t| t != id) } else { self.toggled.push(id.to_string()) }
    }
    /// "Protokoll dieses Bereichs leeren": everything logged in `space`.
    fn clear_space(&mut self, space: &str) {
        let gone: Vec<String> = self.edited.iter().filter(|r| s(r, "space") == space).map(|r| s(r, "id").to_string()).collect();
        self.deleted.retain(|r| s(r, "space") != space);
        self.edited.retain(|r| s(r, "space") != space);
        self.toggled.retain(|t| !gone.contains(t));
    }

    /// The plugin's entries for the menu of a message (`message.menu` payload), as Webox expects them.
    fn menu(&self, ev: &Value, cfg: &Cfg) -> Vec<Value> {
        let (id, space) = (s(ev, "id"), s(ev, "space"));
        let mut v = vec![];
        if id.is_empty() || !cfg.menu { return v; }
        if ev["deleted"] == true && self.is_deleted(id) {
            v.push(json!({ "id": "forget-deleted", "label": "Gelöschte Nachricht entfernen", "icon": "delete-regular", "danger": true }));
        }
        if cfg.edits && self.is_edited(id) {
            let shown = self.history_shown(id, cfg);
            v.push(json!({ "id": "toggle-history", "label": if shown { "Bearbeitungsverlauf ausblenden" } else { "Bearbeitungsverlauf anzeigen" },
                "icon": if shown { "hide-regular" } else { "show-regular" } }));
            v.push(json!({ "id": "forget-history", "label": "Bearbeitungsverlauf entfernen", "icon": "delete-regular", "danger": true }));
        }
        if self.has_space(space) {
            v.push(json!({ "id": "clear-space", "label": "Protokoll dieses Bereichs leeren", "icon": "clear-regular", "danger": true }));
        }
        v
    }

    /// Runs a picked entry (`message.menu_action` payload); whether the log changed.
    fn menu_action(&mut self, ev: &Value) -> bool {
        let (id, space) = (s(ev, "id"), s(ev, "space"));
        match s(ev, "item") {
            "forget-deleted" if self.is_deleted(id) => self.forget_deleted(id),
            "forget-history" if self.is_edited(id) => self.forget_history(id),
            "toggle-history" if self.is_edited(id) => self.toggle_history(id),
            "clear-space" if self.has_space(space) => self.clear_space(space),
            _ => return false,
        }
        true
    }

    fn add_deleted(&mut self, rec: Value) {
        self.deleted.retain(|r| s(r, "id") != s(&rec, "id"));
        self.deleted.push(rec);
        if self.deleted.len() > MAX { self.deleted.remove(0); }
    }

    /// Adds an edit (`message.edited` payload): the earlier texts of that message, oldest first.
    fn add_edit(&mut self, ev: &Value) -> Vec<String> {
        let id = s(ev, "id");
        let i = self.edited.iter().position(|r| s(r, "id") == id);
        let mut rec = i.map(|i| self.edited.remove(i)).unwrap_or_else(|| json!({ "id": id, "history": [] }));
        for k in ["space", "space_name", "author", "mine"] { rec[k] = ev[k].clone(); }
        let mut h: Vec<String> = rec["history"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
        let old = s(ev, "old_text");
        if h.last().map(String::as_str) != Some(old) { h.push(old.to_string()); }
        // A text edited back to an earlier version is no longer an earlier version.
        h.retain(|t| t != s(ev, "new_text"));
        if h.len() > MAX_VERSIONS { h.drain(..h.len() - MAX_VERSIONS); }
        rec["history"] = json!(h);
        self.edited.push(rec);
        if self.edited.len() > MAX { self.edited.remove(0); }
        h
    }

    /// The `message.keep` argument for what `cfg` shows: deleted messages in its style, edit histories, replacing all before.
    fn keep_args(&self, cfg: &Cfg) -> Value {
        let deleted: Vec<Value> = if cfg.deletes {
            self.deleted.iter().filter(|r| cfg.applies(r)).map(|r| { let mut r = r.clone(); r["style"] = json!(cfg.style); r }).collect()
        } else { vec![] };
        let edited: Vec<Value> = if cfg.edits {
            self.edited.iter().filter(|r| cfg.applies(r) && self.history_shown(s(r, "id"), cfg)).map(|r| json!({ "id": r["id"], "history": r["history"] })).collect()
        } else { vec![] };
        json!({ "replace": true, "deleted": deleted, "edited": edited })
    }
}

#[derive(Default)]
pub struct MessageLogger { host: Option<Host>, cfg: Cfg, log: Log }

impl MessageLogger {
    /// The answer to an event (None = Webox handles the message as usual), logging what it keeps.
    fn answer(&mut self, name: &str, payload: &str) -> Option<String> {
        let ev: Value = serde_json::from_str(payload).ok()?;
        match name {
            "message.menu" => { let items = self.log.menu(&ev, &self.cfg); return (!items.is_empty()).then(|| json!({ "items": items }).to_string()); }
            "message.menu_action" => { if self.log.menu_action(&ev) { self.save(); self.sync(); } return None; }
            _ => {}
        }
        if s(&ev, "id").is_empty() || !self.cfg.applies(&ev) { return None; }
        let a = match name {
            "message.deleted" if self.cfg.deletes => { self.log.add_deleted(ev); json!({ "keep": true, "style": self.cfg.style }) }
            "message.edited" if self.cfg.edits && !(self.cfg.ignore_self_edits && ev["mine"] == true) => {
                let h = self.log.add_edit(&ev);
                // Logged either way; shown in the message only when that is on for it.
                json!({ "history": if self.log.history_shown(s(&ev, "id"), &self.cfg) { h } else { vec![] } })
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

    /// Tells Webox what to show now (after a start or a settings change).
    fn sync(&self) {
        if let Some(h) = self.host { h.call("message.keep", &self.log.keep_args(&self.cfg).to_string()); }
    }
}

impl Plugin for MessageLogger {
    const NAME: &'static str = "Nachrichtenprotokoll";
    const AUTHOR: &'static str = "Webox examples";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const DESCRIPTION: &'static str = env!("CARGO_PKG_DESCRIPTION");
    const EVENTS: &'static str = "message.deleted message.edited message.menu message.menu_action";

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
            for t in now.toggled { if !self.log.toggled.contains(&t) { self.log.toggled.push(t); } }
        }
        self.sync();
    }
    fn stop(&mut self) { self.host = None; }

    fn settings(&self) -> Option<Schema> {
        Some(Schema::new()
            .section("Nachrichtenprotokoll", "Gelöschte Nachrichten bleiben sichtbar und werden als gelöscht markiert; bearbeitete Nachrichten zeigen ihre früheren Fassungen. \
                Webox merkt Änderungen beim Laden eines Bereichs. Alles bleibt nur auf diesem Computer und wird nie gesendet.")
            .toggle("deletes", "Gelöschte Nachrichten anzeigen", "", true)
            .toggle("edits", "Bearbeitungen protokollieren", "Frühere Fassungen erscheinen durchgestrichen über dem Text.", true)
            .toggle("ignore_self", "Eigene Nachrichten ignorieren", "", false)
            .toggle("ignore_self_edits", "Eigene Bearbeitungen ignorieren", "Ihre eigenen Bearbeitungen werden nicht protokolliert.", false)
            .toggle("inline_edits", "Bearbeitungsverlauf in der Nachricht anzeigen", "Aus: Bearbeitungen werden protokolliert und pro Nachricht über das Menü eingeblendet.", true)
            .toggle("menu", "Einträge im Nachrichtenmenü", "Rechtsklick auf eine Nachricht: gelöschte Nachricht oder Bearbeitungsverlauf entfernen, Verlauf ein- und ausblenden, Protokoll des Bereichs leeren.", true)
            .choice("style", "Darstellung gelöschter Nachrichten", "", "text", &[("text", "Text rot"), ("overlay", "Überlagerung")])
            .section("Bereiche", "")
            .choice("space_mode", "Bereiche", "", "all", &[("all", "Alle Bereiche"), ("allow", "Nur in diesen Bereichen"), ("deny", "Bereiche ausnehmen")])
            .multiline("spaces", "Liste", "Ein Bereichsname pro Zeile, wie er in Webox steht.", "")
            .section("Speicherung", "Das Protokoll wird nur auf diesem Computer gespeichert und nie gesendet. Ohne die folgende Option liegt es nur im Arbeitsspeicher.")
            .toggle("persist", "Gelöschte Nachrichten auch nach Neustart behalten", "Speichert das Protokoll in den Plugin-Daten von Webox auf diesem Computer.", false)
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
        self.save();
        self.sync();
        Some("Protokoll geleert.".into())
    }

    fn on_event(&mut self, name: &str, payload: &str) -> Option<String> { self.answer(name, payload) }
}

export_plugin!(MessageLogger);

#[cfg(test)]
mod tests;
