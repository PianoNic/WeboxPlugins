//! Translations for Webox's `i18n.strings` event (docs/plugins.md, "Translations"). The code is written in English; each
//! `translations/<lang>.tsv` holds `English<TAB>translation` lines (`\n` for a line break). Webox applies the table to what it
//! draws from the plugin (name, description, settings, menus, dialogs, toasts); [`t`] translates the texts the plugin builds
//! itself. A Webox without the event never sends it, and everything stays English.
use collabhost_plugin_api::{json_str, Host, Values};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

/// (language code, table) for every language this plugin ships.
const TABLES: &[(&str, &str)] = &[("de", include_str!("../translations/de.tsv"))];

/// Index into [`TABLES`] + 1 of the active language; 0 = English.
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

fn parse(tsv: &'static str) -> HashMap<&'static str, String> {
    tsv.lines().filter_map(|l| l.split_once('\t')).filter(|(e, t)| !e.is_empty() && !t.is_empty())
        .map(|(e, t)| (e, t.replace("\\n", "\n"))).collect()
}
fn table(i: usize) -> &'static HashMap<&'static str, String> {
    static PARSED: OnceLock<Vec<HashMap<&'static str, String>>> = OnceLock::new();
    &PARSED.get_or_init(|| TABLES.iter().map(|(_, tsv)| parse(tsv)).collect())[i]
}

/// The table for a language code: an exact match (`de_CH`), else its base language (`de`).
fn pick(lang: &str) -> Option<usize> {
    let base = lang.split(['_', '-']).next().unwrap_or("");
    TABLES.iter().position(|(c, _)| *c == lang).or_else(|| TABLES.iter().position(|(c, _)| *c == base))
}

/// The answer to `i18n.strings` for a language code (`{"strings": {...}}`, empty for a language without a table).
pub(crate) fn answer(lang: &str) -> String {
    let body: Vec<String> = pick(lang).map(table).into_iter().flatten().map(|(e, t)| format!("{}:{}", json_str(e), json_str(t))).collect();
    format!("{{\"strings\":{{{}}}}}", body.join(","))
}

/// Remembers the language of an `{"lang", "fallbacks"}` object for [`t`]; returns the code.
fn remember(payload: &str) -> String {
    let lang = Values::parse(payload).text("lang").unwrap_or("").to_string();
    ACTIVE.store(pick(&lang).map_or(0, |i| i + 1), Ordering::Relaxed);
    lang
}

/// Handles the `i18n.strings` event: remembers the language for [`t`] and returns the answer.
pub fn strings(payload: &str) -> String { answer(&remember(payload)) }

/// Asks Webox for the active language (`i18n.language`), for texts built at run time. A Webox without the method keeps
/// what the last `i18n.strings` event said (or English).
#[allow(dead_code)] // only for plugins that build texts at run time
pub fn sync(host: Host) {
    if let Some(r) = host.call("i18n.language", "{}") { remember(&r); }
}

/// `en` in the active language (unchanged when there is no translation). Placeholders like `{name}` are filled by the caller.
#[allow(dead_code)] // not every plugin builds texts at run time
pub fn t(en: &str) -> String {
    match ACTIVE.load(Ordering::Relaxed) {
        0 => en.to_string(),
        i => table(i - 1).get(en).map_or_else(|| en.to_string(), String::clone),
    }
}

/// The English texts of a settings schema (`Schema::to_json`) that the German table lacks: a test helper that keeps the
/// table complete.
#[cfg(test)]
pub fn untranslated(schema_json: &str, more: &[&str]) -> Vec<String> {
    fn walk(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(m) => for (k, x) in m {
                match x.as_str() { Some(s) if ["title", "description", "label", "filter"].contains(&k.as_str()) => out.push(s.to_string()), _ => walk(x, out) }
            },
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut all = vec![];
    walk(&serde_json::from_str(schema_json).unwrap(), &mut all);
    all.extend(more.iter().map(|s| s.to_string()));
    let de = table(pick("de").unwrap());
    all.into_iter().filter(|s| !s.is_empty() && !de.contains_key(s.as_str())).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn german_is_picked_for_de_and_its_regions_only() {
        assert_eq!(super::pick("de"), Some(0));
        assert_eq!(super::pick("de_CH"), Some(0));
        assert_eq!(super::pick("en"), None);
        assert_eq!(super::answer("fr"), "{\"strings\":{}}");
        let v: serde_json::Value = serde_json::from_str(&super::answer("de")).unwrap();
        assert!(v["strings"].as_object().is_some_and(|m| !m.is_empty() && m.values().all(|x| x.as_str().is_some_and(|s| !s.is_empty()))));
    }
}
