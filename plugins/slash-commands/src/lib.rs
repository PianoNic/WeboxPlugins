mod i18n;

use collabhost_plugin_api::{export_plugin, Host, Plugin, Schema, Values};

const SHRUG: &str = "\u{af}\\_(\u{30c4})_/\u{af}";

struct SlashCommands { host: Option<Host>, sent: u32, smiley: bool, shrug: String }

impl Default for SlashCommands {
    // The same defaults the settings schema declares (Webox sends the saved values before start, these cover the time before).
    fn default() -> Self { SlashCommands { host: None, sent: 0, smiley: true, shrug: SHRUG.into() } }
}

impl Plugin for SlashCommands {
    const NAME: &'static str = "Slash Commands";
    const AUTHOR: &'static str = "Webox examples";
    // Version and description come from Cargo.toml, the one place the repository index reads them from too.
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const DESCRIPTION: &'static str = env!("CARGO_PKG_DESCRIPTION");
    const EVENTS: &'static str = "i18n.strings";
    fn on_event(&mut self, name: &str, payload: &str) -> Option<String> { (name == "i18n.strings").then(|| i18n::strings(payload)) }
    const COMMANDS: &'static str = "shrug tableflip count";

    fn start(&mut self, host: Host) {
        i18n::sync(host);
        self.sent = host.get("sent").and_then(|s| s.parse().ok()).unwrap_or(0);
        self.host = Some(host);
    }
    fn stop(&mut self) { self.host = None; }

    fn settings(&self) -> Option<Schema> {
        Some(Schema::new()
            .section("Replacements", "What is replaced when a message is sent.")
            .toggle("smiley", "Replace \":)\" with \u{1f642}", "Applies to every message you send.", true)
            .section("Commands", "")
            .text("shrug_text", "Text for /shrug", "Sent after the text you write after /shrug.", SHRUG)
            .button("reset_count", "Reset the /count counter", "Sets the number of sent messages to 0."))
    }
    fn settings_changed(&mut self, v: &Values) {
        self.smiley = v.bool("smiley").unwrap_or(true);
        self.shrug = v.text("shrug_text").filter(|s| !s.is_empty()).unwrap_or(SHRUG).to_string();
    }
    fn settings_action(&mut self, id: &str) -> Option<String> {
        (id == "reset_count").then(|| {
            self.sent = 0;
            if let Some(h) = self.host { h.set("sent", "0") }
            i18n::t("Counter reset")
        })
    }

    fn on_send(&mut self, text: &str) -> Option<String> {
        self.sent += 1;
        if let Some(h) = self.host { h.set("sent", &self.sent.to_string()) }
        (self.smiley && text.contains(":)")).then(|| text.replace(":)", "\u{1f642}"))
    }
    fn command(&mut self, name: &str, args: &str) -> Option<String> {
        let tail = if args.is_empty() { String::new() } else { format!("{args} ") };
        match name {
            "shrug" => Some(format!("{tail}{}", self.shrug)),
            "tableflip" => Some(format!("{tail}(\u{256f}\u{b0}\u{25a1}\u{b0})\u{256f}\u{fe35} \u{253b}\u{2501}\u{253b}")),
            "count" => { if let Some(h) = self.host { h.toast(&format!("You sent {} messages", self.sent)) } None }
            _ => None,
        }
    }
}

export_plugin!(SlashCommands);

#[cfg(test)]
mod i18n_tests {
    use super::*;
    #[test]
    fn every_setting_has_a_german_text() {
        let s = SlashCommands::default().settings().unwrap().to_json();
        assert_eq!(i18n::untranslated(&s, &[]), Vec::<String>::new());
    }
    #[test]
    fn the_event_answers_german_and_nothing_for_other_languages() {
        let mut p = SlashCommands::default();
        assert_eq!(p.on_event("i18n.strings", r#"{"lang":"fr","fallbacks":["fr_FR","fr"]}"#).as_deref(), Some("{\"strings\":{}}"));
        assert_eq!(p.on_event("i18n.strings", r#"{"lang":"en","fallbacks":["en_US","en"]}"#).as_deref(), Some("{\"strings\":{}}"));
        let de: serde_json::Value = serde_json::from_str(&i18n::answer("de_CH")).unwrap();
        assert_eq!(de["strings"]["Counter reset"], "Zähler zurückgesetzt");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_and_smiley() {
        let mut p = SlashCommands::default();
        assert_eq!(p.command("shrug", "hi").as_deref(), Some("hi \u{af}\\_(\u{30c4})_/\u{af}"));
        assert_eq!(p.on_send("ok :)").as_deref(), Some("ok \u{1f642}"));
        assert_eq!(p.on_send("plain"), None);
        assert_eq!(p.command("nope", ""), None);
    }

    #[test]
    fn settings_change_the_behaviour() {
        let mut p = SlashCommands::default();
        assert!(p.settings().unwrap().to_json().contains("\"id\":\"shrug_text\""));
        p.settings_changed(&Values::parse(r#"{"smiley":false,"shrug_text":"meh"}"#));
        assert_eq!(p.on_send("ok :)"), None);
        assert_eq!(p.command("shrug", "").as_deref(), Some("meh"));
        p.sent = 5;
        assert_eq!(p.settings_action("reset_count").as_deref(), Some("Counter reset"));
        assert_eq!(p.sent, 0);
        assert_eq!(p.settings_action("other"), None);
    }
}
