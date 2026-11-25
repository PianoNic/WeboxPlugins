//! `sign --repo <dir>`: signs `index.min.json` and `index.json` in the repository folder with the minisign secret key from the
//! environment and writes `<file>.minisig` next to each (the format `minisign -S` writes: Ed25519 over the file's BLAKE2b-512
//! hash). Webox checks `index.min.json.minisig` with the public key built into it (src/extras/plugin_repo/sign.rs in the
//! Webox repository); the index lists every binary's SHA-256 and size, so the signature covers the binaries too.
//!
//! - `MINISIGN_SECRET_KEY`: the whole secret key file (`untrusted comment: ...` + the base64 line).
//! - `MINISIGN_PASSWORD`: its password; empty or unset for a key made without one (`minisign -G -W`).
//!
//! TODO(owner): without `MINISIGN_SECRET_KEY` nothing is signed (and old signatures are removed, so they never sit next to an
//! index they don't match); Webox then shows the repository as unsigned / "not checked". To turn signing on:
//!   1. On a trusted machine: `minisign -G -p webox-plugins.pub -s webox-plugins.key` (choose a password), or with the Rust
//!      tool: `cargo install rsign2 && rsign generate -p webox-plugins.pub -s webox-plugins.key`.
//!   2. Here: Settings > Secrets and variables > Actions > New repository secret: `MINISIGN_SECRET_KEY` = the content of
//!      webox-plugins.key, `MINISIGN_PASSWORD` = its password. Push to main (or run the workflow): the `repo` branch then
//!      carries `index.min.json.minisig`.
//!   3. In the Webox repository, set `OFFICIAL_PUBLIC_KEY` in src/extras/plugin_repo/sign.rs to the second line of
//!      webox-plugins.pub and release Webox.
//!   Keep webox-plugins.key offline and backed up; never commit it.
use std::path::Path;

/// The index files Webox may read, signed in this order.
pub const SIGNED_FILES: [&str; 2] = ["index.min.json", "index.json"];

/// The secret key from `key_text` (a minisign secret key file) and `password` (`None` / empty: an unencrypted key).
pub fn secret_key(key_text: &str, password: Option<&str>) -> Result<minisign::SecretKey, String> {
    let b = minisign::SecretKeyBox::from_string(key_text.trim()).map_err(|e| format!("MINISIGN_SECRET_KEY is not a minisign secret key: {e}"))?;
    match password.filter(|p| !p.is_empty()) {
        Some(p) => minisign::SecretKey::from_box(b, Some(p.to_string())),
        None => minisign::SecretKey::from_unencrypted_box(b),
    }.map_err(|e| format!("the secret key can't be opened (wrong MINISIGN_PASSWORD?): {e}"))
}

/// Signs every index file in `repo` with `sk` (`None`: removes the old signatures instead). Returns the files signed.
pub fn sign_repo(repo: &Path, sk: Option<&minisign::SecretKey>, now: u64) -> Result<Vec<String>, String> {
    let mut done = vec![];
    for name in SIGNED_FILES {
        let (file, sig) = (repo.join(name), repo.join(format!("{name}.minisig")));
        let Some(sk) = sk else { let _ = std::fs::remove_file(&sig); continue };
        if !file.is_file() { continue; }
        let data = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let pk = minisign::PublicKey::from_secret_key(sk).map_err(|e| e.to_string())?;
        let tc = format!("timestamp:{now}\tfile:{name}\thashed");
        let s = minisign::sign(Some(&pk), sk, std::io::Cursor::new(&data), Some(&tc), Some("signature from the WeboxPlugins publish workflow"))
            .map_err(|e| format!("{name}: {e}"))?;
        // check it before publishing it
        minisign::verify(&pk, &s, std::io::Cursor::new(&data), true, false, false).map_err(|e| format!("{name}: the new signature doesn't verify: {e}"))?;
        std::fs::write(&sig, s.to_string()).map_err(|e| format!("{}: {e}", sig.display()))?;
        done.push(name.to_string());
    }
    Ok(done)
}

pub fn cmd(args: &[String]) -> Result<(), String> {
    let repo = std::path::PathBuf::from(super::opt(args, "--repo").ok_or("--repo missing")?);
    let key = std::env::var("MINISIGN_SECRET_KEY").ok().filter(|k| !k.trim().is_empty());
    let Some(key) = key else {
        sign_repo(&repo, None, 0)?;
        println!("warning: MINISIGN_SECRET_KEY is not set: the index is published unsigned (see TODO(owner) in xtask/src/sign.rs)");
        return Ok(());
    };
    let sk = secret_key(&key, std::env::var("MINISIGN_PASSWORD").ok().as_deref())?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let done = sign_repo(&repo, Some(&sk), now)?;
    if done.is_empty() { return Err(format!("no index file in {}", repo.display())); }
    let pk = minisign::PublicKey::from_secret_key(&sk).map_err(|e| e.to_string())?;
    println!("signed {} with public key {}", done.join(", "), pk.to_base64());
    Ok(())
}
