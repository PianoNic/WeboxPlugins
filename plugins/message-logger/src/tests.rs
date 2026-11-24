use super::*;

fn deleted(id: &str, space_name: &str, mine: bool) -> String {
    json!({ "space": format!("sp-{space_name}"), "space_name": space_name, "id": id, "author": "Anna", "email": "anna@example.com", "text": format!("Text {id}"),
        "mine": mine, "parent": "", "published_ms": 1000, "date": "2026-11-24", "time": "09:00" }).to_string()
}
fn edited(id: &str, old: &str, new: &str) -> String {
    json!({ "space": "sp-Team", "space_name": "Team", "id": id, "author": "Anna", "mine": false, "old_text": old, "new_text": new }).to_string()
}
fn with(cfg: Cfg) -> MessageLogger { MessageLogger { cfg, ..Default::default() } }
fn v(t: &str) -> Value { serde_json::from_str(t).unwrap() }

#[test]
fn defaults_match_the_schema() {
    assert_eq!(Cfg::from(&Values::parse("{}")), Cfg::default());
    let schema = v(&MessageLogger::default().settings().unwrap().to_json());
    let mut defaults = serde_json::Map::new();
    for f in schema["sections"].as_array().unwrap().iter().flat_map(|s| s["fields"].as_array().unwrap()) {
        if let Some(d) = f.get("default") { defaults.insert(f["id"].as_str().unwrap().into(), d.clone()); }
    }
    assert_eq!(Cfg::from(&Values::parse(&Value::Object(defaults).to_string())), Cfg::default());
    // Shown deleted messages and edits are on, restart persistence off.
    let d = Cfg::default();
    assert!(d.deletes && d.edits && !d.ignore_self && !d.persist && d.style == "text" && d.space_mode == "all");
    let c = Cfg::from(&Values::parse(r#"{"style":"overlay","space_mode":"weird","spaces":" Team \n\n Projekt X "}"#));
    assert_eq!((c.style.as_str(), c.space_mode.as_str(), c.spaces.clone()), ("overlay", "all", vec!["team".to_string(), "projekt x".to_string()]));
}

#[test]
fn deleted_messages_are_kept_in_the_chosen_style() {
    let mut p = with(Cfg::default());
    assert_eq!(p.on_event("message.deleted", &deleted("m1", "Team", false)).map(|a| v(&a)), Some(json!({ "keep": true, "style": "text" })));
    let mut p2 = with(Cfg { style: "overlay".into(), ..Cfg::default() });
    assert_eq!(p2.on_event("message.deleted", &deleted("m1", "Team", false)).map(|a| v(&a)), Some(json!({ "keep": true, "style": "overlay" })));
    // The payload is logged as delivered (it is what message.keep needs after a restart); the same id once.
    p.on_event("message.deleted", &deleted("m1", "Team", false));
    assert_eq!(p.log.deleted, vec![v(&deleted("m1", "Team", false))]);
    // Switched off, unreadable or without id: Webox deletes it as usual.
    let mut off = with(Cfg { deletes: false, ..Cfg::default() });
    assert_eq!(off.on_event("message.deleted", &deleted("m1", "Team", false)), None);
    assert!(off.log.deleted.is_empty());
    assert_eq!(p.on_event("message.deleted", "kaputt"), None);
    assert_eq!(p.on_event("message.deleted", r#"{"space":"s"}"#), None);
    assert_eq!(p.on_event("other.event", &deleted("m2", "Team", false)), None);
}

#[test]
fn own_messages_and_spaces_are_filtered() {
    let mut p = with(Cfg { ignore_self: true, ..Cfg::default() });
    assert_eq!(p.on_event("message.deleted", &deleted("m1", "Team", true)), None);
    assert!(p.on_event("message.deleted", &deleted("m2", "Team", false)).is_some());
    let allow = Cfg { space_mode: "allow".into(), spaces: vec!["team".into()], ..Cfg::default() };
    let mut p = with(allow.clone());
    assert!(p.on_event("message.deleted", &deleted("m1", "TEAM", false)).is_some(), "names compare case-insensitively");
    assert_eq!(p.on_event("message.deleted", &deleted("m2", "Andere", false)), None);
    // A line may also be the space id.
    let mut p = with(Cfg { space_mode: "allow".into(), spaces: vec!["sp-andere".into()], ..Cfg::default() });
    assert!(p.on_event("message.deleted", &deleted("m3", "Andere", false)).is_some());
    let mut p = with(Cfg { space_mode: "deny".into(), ..allow });
    assert_eq!(p.on_event("message.deleted", &deleted("m1", "Team", false)), None);
    assert!(p.on_event("message.deleted", &deleted("m2", "Andere", false)).is_some());
    assert_eq!(p.on_event("message.edited", &edited("m3", "a", "b")), None, "edits in Team are left out too");
}

#[test]
fn edits_collect_earlier_versions() {
    let mut p = with(Cfg::default());
    let h = |a: Option<String>| v(&a.unwrap())["history"].clone();
    assert_eq!(h(p.on_event("message.edited", &edited("m1", "eins", "eins!"))), json!(["eins"]));
    assert_eq!(h(p.on_event("message.edited", &edited("m1", "eins!", "eins?"))), json!(["eins", "eins!"]));
    // The same edit again adds nothing; edited back to the first text: that one is current again.
    assert_eq!(h(p.on_event("message.edited", &edited("m1", "eins!", "eins?"))), json!(["eins", "eins!"]));
    assert_eq!(h(p.on_event("message.edited", &edited("m1", "eins?", "eins"))), json!(["eins!", "eins?"]));
    assert_eq!(p.log.edited.len(), 1);
    assert_eq!(p.log.edited[0]["space_name"], "Team");
    let mut off = with(Cfg { edits: false, ..Cfg::default() });
    assert_eq!(off.on_event("message.edited", &edited("m1", "a", "b")), None);
    // At most MAX_VERSIONS earlier texts.
    for i in 0..30 { p.on_event("message.edited", &edited("m2", &format!("v{i}"), &format!("v{}", i + 1))); }
    let last = h(p.on_event("message.edited", &edited("m2", "v30", "v31")));
    assert_eq!(last.as_array().unwrap().len(), MAX_VERSIONS);
    assert_eq!(last[MAX_VERSIONS - 1], "v30");
}

#[test]
fn the_log_is_bounded_and_survives_a_round_trip() {
    let mut p = with(Cfg::default());
    for i in 0..MAX + 5 { p.on_event("message.deleted", &deleted(&format!("m{i}"), "Team", false)); }
    assert_eq!(p.log.deleted.len(), MAX);
    assert_eq!(s(&p.log.deleted[0], "id"), "m5");
    p.on_event("message.edited", &edited("e1", "a", "b"));
    let back = Log::parse(&p.log.to_json());
    assert_eq!(back, p.log);
    assert_eq!(Log::parse(""), Log::default());
    assert_eq!(Log::parse(r#"{"deleted":[{"text":"no id"}],"edited":"x"}"#), Log::default());
}

#[test]
fn keep_args_follow_the_settings() {
    let mut p = with(Cfg::default());
    p.on_event("message.deleted", &deleted("m1", "Team", false));
    p.on_event("message.deleted", &deleted("m2", "Privat", true));
    p.on_event("message.edited", &edited("m3", "a", "b"));
    let a = p.log.keep_args(&Cfg { style: "overlay".into(), ..Cfg::default() });
    assert_eq!(a["replace"], true);
    assert_eq!(a["deleted"].as_array().unwrap().iter().map(|r| (s(r, "id"), s(r, "style"))).collect::<Vec<_>>(), [("m1", "overlay"), ("m2", "overlay")]);
    assert_eq!(a["deleted"][0]["text"], "Text m1");
    assert_eq!(a["edited"], json!([{ "id": "m3", "history": ["a"] }]));
    // Own messages ignored now, edits off: what Webox shows shrinks accordingly (the log itself keeps everything).
    let a = p.log.keep_args(&Cfg { ignore_self: true, edits: false, ..Cfg::default() });
    assert_eq!(a["deleted"].as_array().unwrap().len(), 1);
    assert_eq!(a["edited"], json!([]));
    let a = p.log.keep_args(&Cfg { deletes: false, ..Cfg::default() });
    assert_eq!(a["deleted"], json!([]));
    // "Protokoll leeren".
    assert_eq!(p.settings_action("clear").as_deref(), Some("Protokoll geleert."));
    assert_eq!(p.log, Log::default());
    assert_eq!(p.settings_action("other"), None);
}
