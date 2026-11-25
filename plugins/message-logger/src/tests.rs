use super::*;

fn deleted(id: &str, space_name: &str, mine: bool) -> String {
    json!({ "space": format!("sp-{space_name}"), "space_name": space_name, "team": "t1", "team_name": "Projekt", "id": id, "author": "Anna", "email": "anna@example.com",
        "text": format!("Text {id}"), "mine": mine, "parent": "", "published_ms": 1000, "date": "2026-11-24", "time": "09:00" }).to_string()
}
fn edited(id: &str, old: &str, new: &str) -> String {
    json!({ "space": "sp-Team", "space_name": "Team", "team": "", "team_name": "", "id": id, "author": "Anna", "email": "anna@example.com", "mine": false,
        "published_ms": 1000, "old_edited": false, "old_text": old, "new_text": new }).to_string()
}
fn with(cfg: Cfg) -> MessageLogger { MessageLogger { cfg, ..Default::default() } }
fn v(t: &str) -> Value { serde_json::from_str(t).unwrap() }
fn ev(p: &mut MessageLogger, name: &str, payload: Value) -> Option<Value> { p.on_event(name, &payload.to_string()).map(|a| v(&a)) }

#[test]
fn defaults_match_the_schema_and_equicord() {
    assert_eq!(Cfg::from(&Values::parse("{}")), Cfg::default());
    let schema = v(&MessageLogger::default().settings().unwrap().to_json());
    let mut defaults = serde_json::Map::new();
    let mut ids = vec![];
    for f in schema["sections"].as_array().unwrap().iter().flat_map(|s| s["fields"].as_array().unwrap()) {
        ids.push(f["id"].as_str().unwrap().to_string());
        if let Some(d) = f.get("default") { defaults.insert(f["id"].as_str().unwrap().into(), d.clone()); }
    }
    assert_eq!(Cfg::from(&Values::parse(&Value::Object(defaults).to_string())), Cfg::default());
    // Every Equicord setting, under its Equicord name.
    for k in ["deleteStyle", "logDeletes", "collapseDeleted", "logEdits", "inlineEdits", "logDeletedAttachments", "ignoreBots", "ignoreSelf", "ignoreSelfEdits",
        "ignoreUsers", "ignoreChannels", "ignoreGuilds", "showEditDiffs", "separatedDiffs"] { assert!(ids.iter().any(|i| i == k), "{k}"); }
    // Equicord's defaults: red text, log both, bots ignored, no diffs.
    let d = Cfg::default();
    assert!(d.log_deletes && d.log_edits && d.inline && d.attachments && d.ignore_bots && !d.ignore_self && !d.collapse && !d.diffs && !d.separated && d.style == "text");
    let c = Cfg::from(&Values::parse(r#"{"deleteStyle":"overlay","ignoreChannels":" Team \n\n Projekt X, sp-1 ","separatedDiffs":true}"#));
    assert_eq!((c.style.as_str(), c.spaces.clone()), ("overlay", vec!["team".to_string(), "projekt x".to_string(), "sp-1".to_string()]));
    assert!(!c.separated, "separated diffs only with diffs");
}

#[test]
fn deleted_messages_are_kept_as_the_settings_say() {
    let mut p = with(Cfg::default());
    assert_eq!(p.on_event("message.deleted", &deleted("m1", "Team", false)).map(|a| v(&a)),
        Some(json!({ "keep": true, "style": "text", "collapsed": false, "attachments": true })));
    let mut p2 = with(Cfg { style: "overlay".into(), collapse: true, attachments: false, ..Cfg::default() });
    assert_eq!(p2.on_event("message.deleted", &deleted("m1", "Team", false)).map(|a| v(&a)),
        Some(json!({ "keep": true, "style": "overlay", "collapsed": true, "attachments": false })));
    p.on_event("message.deleted", &deleted("m1", "Team", false));
    assert_eq!(p.log.deleted, vec![v(&deleted("m1", "Team", false))]);
    let mut off = with(Cfg { log_deletes: false, ..Cfg::default() });
    assert_eq!(off.on_event("message.deleted", &deleted("m1", "Team", false)), None);
    assert!(off.log.deleted.is_empty());
    assert_eq!(p.on_event("message.deleted", "kaputt"), None);
    assert_eq!(p.on_event("message.deleted", r#"{"space":"s"}"#), None);
}

#[test]
fn should_ignore_follows_equicord() {
    let base = v(&deleted("m1", "Team", false));
    let c = Cfg::default();
    assert!(!c.ignores(&base, false));
    let mut bot = base.clone();
    bot["email"] = json!("helper@webex.bot");
    assert!(c.ignores(&bot, false), "bots by default");
    assert!(!Cfg { ignore_bots: false, ..Cfg::default() }.ignores(&bot, false));
    let mut mine = base.clone();
    mine["mine"] = json!(true);
    assert!(Cfg { ignore_self: true, ..Cfg::default() }.ignores(&mine, false));
    let self_edits = Cfg { ignore_self_edits: true, ..Cfg::default() };
    assert!(self_edits.ignores(&mine, true) && !self_edits.ignores(&mine, false), "only own edits");
    assert!(Cfg { users: vec!["anna".into()], ..Cfg::default() }.ignores(&base, false));
    assert!(Cfg { users: vec!["anna@example.com".into()], ..Cfg::default() }.ignores(&base, false));
    assert!(Cfg { spaces: vec!["team".into()], ..Cfg::default() }.ignores(&base, false));
    assert!(Cfg { spaces: vec!["sp-team".into()], ..Cfg::default() }.ignores(&base, false), "a space id");
    assert!(Cfg { teams: vec!["projekt".into()], ..Cfg::default() }.ignores(&base, false), "a team name");
    assert!(Cfg { teams: vec!["t1".into()], ..Cfg::default() }.ignores(&base, false), "a team id");
    assert!(!Cfg { teams: vec!["".into()], ..Cfg::default() }.ignores(&v(&edited("m2", "a", "b")), true), "no team: never an empty match");
    assert!(Cfg { log_edits: false, ..Cfg::default() }.ignores(&base, true) && !Cfg { log_edits: false, ..Cfg::default() }.ignores(&base, false));
}

#[test]
fn edits_collect_versions_with_their_times() {
    let mut p = with(Cfg::default());
    let h = |a: Option<String>| v(&a.unwrap())["history"].clone();
    assert_eq!(h(p.on_event("message.edited", &edited("m1", "one", "one!"))), json!(["one"]));
    assert_eq!(h(p.on_event("message.edited", &edited("m1", "one!", "one?"))), json!(["one", "one!"]));
    assert_eq!(h(p.on_event("message.edited", &edited("m1", "one!", "one?"))), json!(["one", "one!"]));
    assert_eq!(h(p.on_event("message.edited", &edited("m1", "one?", "one"))), json!(["one!", "one?"]));
    let r = &p.log.edited[0];
    assert_eq!((r["current"].as_str(), r["published_ms"].as_i64()), (Some("one"), Some(1000)));
    assert!(r["history"].as_array().unwrap().iter().all(|x| x["ms"].as_i64().unwrap() > 0));
    for i in 0..30 { p.on_event("message.edited", &edited("m2", &format!("v{i}"), &format!("v{}", i + 1))); }
    assert_eq!(h(p.on_event("message.edited", &edited("m2", "v30", "v31"))).as_array().unwrap().len(), MAX_VERSIONS);
    // The first text Webox saw was already an edit: its time is not known.
    let mut q = with(Cfg::default());
    let mut e = v(&edited("m3", "b", "c"));
    e["old_edited"] = json!(true);
    q.on_event("message.edited", &e.to_string());
    assert_eq!(q.log.edited[0]["history"][0]["ms"], 0);
    // Off: nothing logged.
    assert_eq!(with(Cfg { log_edits: false, ..Cfg::default() }).on_event("message.edited", &edited("m1", "a", "b")), None);
}

#[test]
fn inline_and_diff_views() {
    // inlineEdits off: logged, only the tag.
    let mut p = with(Cfg { inline: false, ..Cfg::default() });
    assert_eq!(p.on_event("message.edited", &edited("m1", "Hello world", "Hello folks")).map(|a| v(&a)), Some(json!({ "history": [], "inline": false })));
    assert_eq!(p.log.edited.len(), 1);
    // showEditDiffs: each earlier text against the next one.
    let mut p = with(Cfg { diffs: true, ..Cfg::default() });
    p.on_event("message.edited", &edited("m1", "watch", "watch this"));
    let a = ev(&mut p, "message.edited", v(&edited("m1", "watch this", "watch this now"))).unwrap();
    assert_eq!(a["history"], json!([[{ "text": "watch" }, { "text": " this", "kind": "added" }], [{ "text": "watch this" }, { "text": " now", "kind": "added" }]]));
    assert!(a.get("current").is_none());
    // separatedDiffs: the original with what was removed, the current text with what was added.
    let mut p = with(Cfg { diffs: true, separated: true, ..Cfg::default() });
    let a = ev(&mut p, "message.edited", v(&edited("m1", "prefix watch", "watch it"))).unwrap();
    assert_eq!(a["history"], json!([[{ "text": "prefix ", "kind": "removed" }, { "text": "watch" }]]));
    assert_eq!(a["current"], json!([{ "text": "watch" }, { "text": " it", "kind": "added" }]));
}

#[test]
fn the_message_menu_matches_equicord() {
    let mut p = with(Cfg::default());
    let labels = |p: &mut MessageLogger, id: &str, del: bool| ev(p, "message.menu", json!({ "space": "sp-Team", "id": id, "deleted": del, "edited": !del, "mine": false }))
        .map(|a| a["items"].as_array().unwrap().iter().map(|i| i["label"].as_str().unwrap().to_string()).collect::<Vec<_>>()).unwrap_or_default();
    assert!(labels(&mut p, "x", false).is_empty(), "no history: nothing added");
    p.on_event("message.deleted", &deleted("d1", "Team", false));
    p.on_event("message.edited", &edited("e1", "a", "b"));
    assert_eq!(labels(&mut p, "d1", true), ["Toggle Deleted Highlight", "Clear Message History"]);
    assert_eq!(labels(&mut p, "e1", false), ["Clear Message History"]);
    p.cfg.diffs = true;
    assert_eq!(labels(&mut p, "e1", false), ["Disable Diff View", "Clear Message History"]);
    // Toggles are per message and session: the diff view goes, the highlight goes ("plain").
    ev(&mut p, "message.menu_action", json!({ "space": "sp-Team", "id": "e1", "item": "toggle-diff" }));
    assert_eq!(labels(&mut p, "e1", false), ["Enable Diff View", "Clear Message History"]);
    assert_eq!(p.keep_args()["edited"][0]["history"], json!(["a"]), "no diff for it now");
    ev(&mut p, "message.menu_action", json!({ "space": "sp-Team", "id": "d1", "item": "toggle-deleted" }));
    assert_eq!(p.keep_args()["deleted"][0]["style"], "plain");
    ev(&mut p, "message.menu_action", json!({ "space": "sp-Team", "id": "d1", "item": "toggle-deleted" }));
    assert_eq!(p.keep_args()["deleted"][0]["style"], "text");
    // Clear Message History: a deleted message is forgotten, an edited one loses its history.
    ev(&mut p, "message.menu_action", json!({ "space": "sp-Team", "id": "d1", "item": "clear-history" }));
    ev(&mut p, "message.menu_action", json!({ "space": "sp-Team", "id": "e1", "item": "clear-history" }));
    assert!(p.log.deleted.is_empty() && p.log.edited.is_empty());
    assert_eq!(p.keep_args(), json!({ "replace": true, "deleted": [], "edited": [] }));
}

#[test]
fn the_space_menu_clears_that_spaces_log() {
    let mut p = with(Cfg::default());
    p.on_event("message.deleted", &deleted("d1", "Team", false));
    p.on_event("message.deleted", &deleted("d2", "Other", false));
    assert_eq!(ev(&mut p, "space.menu", json!({ "space": "sp-Empty" })), None, "only where something is logged");
    assert_eq!(ev(&mut p, "space.menu", json!({ "space": "sp-Team" })), Some(json!({ "items": [{ "id": "clear-log", "label": "Clear Message Log", "danger": true }] })));
    ev(&mut p, "space.menu_action", json!({ "space": "sp-Team", "item": "clear-log" }));
    assert_eq!(p.log.deleted.iter().map(|r| s(r, "id")).collect::<Vec<_>>(), ["d2"]);
}

#[test]
fn the_history_dialog_has_a_tab_per_version() {
    let mut p = with(Cfg { diffs: true, ..Cfg::default() });
    p.on_event("message.edited", &edited("m1", "Hello", "Hello world"));
    p.on_event("message.edited", &edited("m1", "Hello world", "Hello folks"));
    let d = p.history_dialog(&json!({ "space": "sp-Team", "id": "m1", "text": "Hello folks", "published_ms": 1000 }));
    assert_eq!(d["title"], "Message Edit History");
    assert_eq!(d["switch"], json!({ "label": "Show Diff", "on": true }));
    let tabs = d["tabs"].as_array().unwrap();
    assert_eq!(tabs.len(), 3, "two earlier versions and the current one");
    assert_eq!(d["selected"], 2);
    assert_eq!((tabs[0]["time_ms"].as_i64(), tabs[0]["text"].as_str()), (Some(1000), Some("Hello")));
    assert_eq!(tabs[0]["alt"], json!([{ "text": "Hello" }, { "text": " folks", "kind": "added" }]), "against the newest version");
    assert_eq!(tabs[2]["text"], "Hello folks");
    assert!(tabs[2]["time_ms"].as_i64().unwrap() > 0 && tabs[2].get("alt").is_none());
    // Nothing logged (or the original was never seen): a greyed tab for the original, then the current text.
    let d = p.history_dialog(&json!({ "space": "sp-Team", "id": "zz", "text": "Now", "published_ms": 500 }));
    let tabs = d["tabs"].as_array().unwrap();
    assert_eq!((tabs.len(), tabs[0]["disabled"].as_bool(), tabs[1]["text"].as_str()), (2, Some(true), Some("Now")));
    // A message whose diff view was switched off: no diff in the dialog either.
    ev(&mut p, "message.menu_action", json!({ "space": "sp-Team", "id": "m1", "item": "toggle-diff" }));
    let d = p.history_dialog(&json!({ "space": "sp-Team", "id": "m1", "text": "Hello folks", "published_ms": 1000 }));
    assert!(d["tabs"][0].get("alt").is_none() && d["switch"]["on"] == false);
}

#[test]
fn settings_changes_apply_to_what_is_shown() {
    let mut p = with(Cfg::default());
    p.on_event("message.deleted", &deleted("m1", "Team", false));
    p.on_event("message.edited", &edited("m2", "old", "new"));
    let k = p.keep_args();
    assert_eq!(k["deleted"][0]["style"], "text");
    assert_eq!(k["edited"], json!([{ "id": "m2", "history": ["old"] }]));
    p.cfg = Cfg { style: "overlay".into(), collapse: true, attachments: false, ..Cfg::default() };
    let k = p.keep_args();
    assert_eq!((k["deleted"][0]["style"].as_str(), k["deleted"][0]["collapsed"].as_bool(), k["deleted"][0]["attachments"].as_bool()), (Some("overlay"), Some(true), Some(false)));
    p.cfg = Cfg { log_deletes: false, spaces: vec!["team".into()], ..Cfg::default() };
    assert_eq!(p.keep_args(), json!({ "replace": true, "deleted": [], "edited": [] }));
}

#[test]
fn a_1_1_log_still_loads() {
    let l = Log::parse(r#"{"deleted":[{"id":"d1","space":"s"}],"edited":[{"id":"e1","space":"s","history":["one","two"]}],"toggled":["e1"]}"#);
    assert_eq!(l.edited[0]["history"], json!([{ "text": "one", "ms": 0 }, { "text": "two", "ms": 0 }]));
    assert_eq!(Log::parse(&l.to_json()), l);
    assert_eq!(Log::parse("kaputt"), Log::default());
}
