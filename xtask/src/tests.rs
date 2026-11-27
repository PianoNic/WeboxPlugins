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

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("xtask-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The signatures `sign` writes verify with the library Webox checks them with, and only for the exact index bytes.
#[test]
fn signed_index_verifies_like_webox_checks_it() {
    let d = tmp("sign");
    std::fs::write(d.join("index.min.json"), r#"{"plugins":[]}"#).unwrap();
    std::fs::write(d.join("index.json"), "{\n  \"plugins\": []\n}\n").unwrap();
    // a password-protected key, as `minisign -G` makes it, read back from its file text like the CI secret
    let kp = minisign::KeyPair::generate_encrypted_keypair(Some("pw".into())).unwrap();
    let key_text = kp.sk.to_box(None).unwrap().into_string();
    assert!(sign::secret_key(&key_text, Some("wrong")).is_err());
    let sk = sign::secret_key(&key_text, Some("pw")).unwrap();
    assert_eq!(sign::sign_repo(&d, Some(&sk), 1_764_000_000).unwrap(), ["index.min.json", "index.json"]);
    let pk = minisign_verify::PublicKey::from_base64(&kp.pk.to_base64()).unwrap();
    for name in sign::SIGNED_FILES {
        let sig = minisign_verify::Signature::decode(&std::fs::read_to_string(d.join(format!("{name}.minisig"))).unwrap()).unwrap();
        assert!(sig.trusted_comment().contains(&format!("file:{name}")));
        let data = std::fs::read(d.join(name)).unwrap();
        pk.verify(&data, &sig, false).unwrap();
        let mut changed = data.clone();
        changed.push(b' ');
        assert!(pk.verify(&changed, &sig, false).is_err(), "{name} changed after signing");
    }
    // another key doesn't verify
    let other = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
    let other_pk = minisign_verify::PublicKey::from_base64(&other.pk.to_base64()).unwrap();
    let sig = minisign_verify::Signature::decode(&std::fs::read_to_string(d.join("index.min.json.minisig")).unwrap()).unwrap();
    assert!(other_pk.verify(&std::fs::read(d.join("index.min.json")).unwrap(), &sig, false).is_err());
    // an unencrypted key (`minisign -G -W`) works without a password
    let plain = other.sk.to_box(None).unwrap().into_string();
    assert!(sign::secret_key(&plain, None).is_ok() && sign::secret_key(&plain, Some("")).is_ok());
    // without a key: nothing signed, and old signatures go (they would not match the new index)
    assert!(sign::sign_repo(&d, None, 0).unwrap().is_empty());
    assert!(!d.join("index.min.json.minisig").exists() && !d.join("index.json.minisig").exists());
    let _ = std::fs::remove_dir_all(&d);
}
