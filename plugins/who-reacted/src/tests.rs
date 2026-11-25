use super::*;
use serde_json::{json, Value};

fn args(c: &Cfg) -> Value { serde_json::from_str(&c.args()).unwrap() }

#[test]
fn defaults_match_the_schema() {
    // What Webox passes before the first start when nothing was changed: the schema's defaults.
    let s: Value = serde_json::from_str(&WhoReacted::default().settings().unwrap().to_json()).unwrap();
    let defaults: serde_json::Map<String, Value> = s["sections"][0]["fields"].as_array().unwrap().iter()
        .map(|f| (f["id"].as_str().unwrap().to_string(), f["default"].clone())).collect();
    assert_eq!(Cfg::from(&Values::parse(&Value::Object(defaults).to_string())), Cfg::default());
    assert_eq!(args(&Cfg::default()), json!({ "enabled": true, "max": 3, "size": "small", "hide_self": false }));
}

#[test]
fn schema_has_the_four_german_settings() {
    let s = WhoReacted::default().settings().unwrap().to_json();
    for label in ["Show avatars", "Show at most", "Size", "Small", "Medium", "Hide your own reaction"] { assert!(s.contains(label), "{label}"); }
    let v: Value = serde_json::from_str(&s).unwrap();
    let max = v["sections"][0]["fields"].as_array().unwrap().iter().find(|f| f["id"] == "max").unwrap().clone();
    assert_eq!((max["min"].as_f64(), max["max"].as_f64(), max["step"].as_f64()), (Some(1.0), Some(10.0), Some(1.0)));
}

#[test]
fn settings_become_the_method_argument() {
    let c = Cfg::from(&Values::parse(r#"{"on":false,"max":7,"size":"medium","hide_self":true}"#));
    assert_eq!(args(&c), json!({ "enabled": false, "max": 7, "size": "medium", "hide_self": true }));
    // Out of range or unknown values never reach the host as such.
    let c = Cfg::from(&Values::parse(r#"{"max":42.4,"size":"riesig"}"#));
    assert_eq!((c.max, c.size), (10, "small"));
    assert_eq!(Cfg::from(&Values::parse(r#"{"max":0}"#)).max, 1);
    assert_eq!(Cfg::from(&Values::parse("kaputt")), Cfg::default());
}

#[test]
fn host_errors_are_read() {
    assert_eq!(error_of(r#"{"error":"size: small or medium"}"#).as_deref(), Some("size: small or medium"));
    assert_eq!(error_of("{}"), None);
    assert_eq!(error_of("not json"), None);
}

#[test]
fn not_started_it_calls_nothing() {
    // Settings arrive once before start: without a host they are only stored.
    let mut p = WhoReacted::default();
    p.settings_changed(&Values::parse(r#"{"max":5}"#));
    assert_eq!(p.cfg.max, 5);
    assert!(p.host.is_none());
}
