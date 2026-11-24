//! Wer hat reagiert: small round avatars of the people who reacted, right after each reaction chip's count, up to a limit
//! and then "+N", with the name on hover (like Equicord's WhoReacted). Webox draws them; the plugin turns them on and
//! configures them through the host method `reactions.show_avatars`, in `start` and on every settings change.
use collabhost_plugin_api::{export_plugin, Host, Plugin, Schema, Values};

/// The host method this plugin drives.
const METHOD: &str = "reactions.show_avatars";

#[derive(Clone, Debug, PartialEq)]
struct Cfg {
    on: bool,
    /// At most this many avatars per reaction (1..=10).
    max: u32,
    /// "small" or "medium".
    size: &'static str,
    hide_self: bool,
}
impl Default for Cfg {
    // The same defaults the schema declares.
    fn default() -> Self { Cfg { on: true, max: 3, size: "small", hide_self: false } }
}
impl Cfg {
    fn from(v: &Values) -> Cfg {
        let d = Cfg::default();
        Cfg {
            on: v.bool("on").unwrap_or(d.on),
            max: v.number("max").map_or(d.max, |n| n.round().clamp(1.0, 10.0) as u32),
            size: if v.text("size") == Some("medium") { "medium" } else { "small" },
            hide_self: v.bool("hide_self").unwrap_or(d.hide_self),
        }
    }
    /// The argument of `reactions.show_avatars`.
    fn args(&self) -> String {
        serde_json::json!({ "enabled": self.on, "max": self.max, "size": self.size, "hide_self": self.hide_self }).to_string()
    }
}

/// The `error` of a host answer, if it has one.
fn error_of(answer: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(answer).ok()?.get("error")?.as_str().map(String::from)
}

#[derive(Default)]
pub struct WhoReacted { host: Option<Host>, cfg: Cfg }

impl WhoReacted {
    /// Tells the host the current settings (once started); a refusal goes to the plugin log.
    fn apply(&self) {
        let Some(h) = self.host else { return };
        if let Some(e) = h.call(METHOD, &self.cfg.args()).as_deref().and_then(error_of) { h.log(&format!("{METHOD}: {e}")); }
    }
}

impl Plugin for WhoReacted {
    const NAME: &'static str = "Wer hat reagiert";
    const AUTHOR: &'static str = "Webox examples";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const DESCRIPTION: &'static str = env!("CARGO_PKG_DESCRIPTION");

    fn start(&mut self, host: Host) {
        if !host.has(METHOD) {
            host.toast("Diese Webox-Version kann keine Avatare an Reaktionen zeigen (reactions.show_avatars fehlt). Bitte Webox aktualisieren.");
        }
        self.host = Some(host);
        self.apply();
    }
    // Switched off, Webox stops drawing the avatars by itself.
    fn stop(&mut self) { self.host = None; }

    fn settings(&self) -> Option<Schema> {
        Some(Schema::new()
            .section("Wer hat reagiert", "Zeigt neben jeder Reaktion kleine Avatare der Personen, die so reagiert haben. Der Name erscheint beim Darüberfahren.")
            .toggle("on", "Avatare anzeigen", "", true)
            .number("max", "Höchstens anzeigen", "Weitere Personen erscheinen als „+N“.", 3.0, 1.0, 10.0, 1.0)
            .choice("size", "Größe", "", "small", &[("small", "Klein"), ("medium", "Mittel")])
            .toggle("hide_self", "Eigene Reaktion ausblenden", "Ihr eigener Avatar erscheint nicht.", false))
    }
    fn settings_changed(&mut self, v: &Values) {
        self.cfg = Cfg::from(v);
        self.apply();
    }
}

export_plugin!(WhoReacted);

#[cfg(test)]
mod tests;
