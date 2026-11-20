use super::*;
use serde_json::json;

fn meta(pkg: &str, version: &str) -> PluginMeta {
    PluginMeta { name: "Test".into(), pkg: pkg.into(), version: version.into(), description: "d".into(), authors: vec!["Alex Example".into()],
        homepage: None, icon: None, min_webox: None, lib: "collabhost_test".into() }
}
fn built(platform: &str, pkg: &str, version: &str, contract: &str) -> Built {
    Built { platform: platform.into(), pkg: pkg.into(), version: version.into(), contract: contract.into(),
        file: format!("files/{pkg}/{version}/{platform}/x"), sha256: "a".repeat(64), size: 3, dir: PathBuf::new() }
}

#[test]
fn index_merges_platforms() {
    let idx = build_index(&[meta("dev.webox.b", "1.0.0"), meta("dev.webox.a", "2.0.0")],
        &[built("windows-x86_64", "dev.webox.a", "2.0.0", "1.1"), built("linux-x86_64", "dev.webox.a", "2.0.0", "1.1"), built("linux-x86_64", "dev.webox.b", "1.0.0", "1.0")]).unwrap();
    let p = idx["plugins"].as_array().unwrap();
    assert_eq!(p[0]["pkg"], "dev.webox.a");
    assert_eq!(p[0]["files"].as_object().unwrap().len(), 2);
    assert_eq!(p[0]["files"]["linux-x86_64"]["file"], "files/dev.webox.a/2.0.0/linux-x86_64/x");
    assert_eq!(p[1]["contract"], "1.0");
    assert_eq!(idx["formatVersion"], 1);
}

#[test]
fn index_refuses_inconsistent_builds() {
    assert!(build_index(&[meta("dev.webox.a", "1.0.0")], &[]).is_err());
    assert!(build_index(&[meta("dev.webox.a", "1.0.0")], &[built("linux-x86_64", "dev.webox.a", "0.9.0", "1.1")]).is_err());
    assert!(build_index(&[meta("dev.webox.a", "1.0.0")], &[built("linux-x86_64", "dev.webox.a", "1.0.0", "1.1"), built("windows-x86_64", "dev.webox.a", "1.0.0", "1.0")]).is_err());
}

#[test]
fn metadata_rules() {
    let pkg = |name: &str, pkgid: &str, version: &str| json!({ "name": name, "version": version, "description": "d", "authors": ["Alex Example <alex@example.com>"],
        "manifest_path": "/x/Cargo.toml", "metadata": { "webox": { "pkg": pkgid } }, "targets": [{ "name": name, "crate_types": ["cdylib"] }] });
    let ok = meta::from_metadata(&json!({ "packages": [pkg("collabhost_a", "dev.webox.a", "1.0.0"), { "name": "xtask", "metadata": null }] })).unwrap();
    assert_eq!((ok.len(), ok[0].authors[0].as_str(), ok[0].lib.as_str()), (1, "Alex Example", "collabhost_a"));
    assert!(meta::from_metadata(&json!({ "packages": [pkg("a", "Dev.Webox", "1.0.0")] })).is_err());
    assert!(meta::from_metadata(&json!({ "packages": [pkg("a", "dev.webox.a", "1.0")] })).is_err());
    assert!(meta::from_metadata(&json!({ "packages": [pkg("a", "dev.webox.a", "1.0.0"), pkg("b", "dev.webox.a", "1.0.0")] })).is_err());
}

#[test]
fn platform_key_matches_webox() {
    let k = host_platform();
    assert!(["windows-x86_64", "linux-x86_64", "macos-aarch64", "macos-x86_64", "linux-aarch64", "windows-aarch64"].contains(&k.as_str()), "{k}");
}
