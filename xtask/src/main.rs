//! Repository tooling (`cargo run -p xtask -- <command>`):
//!
//! - `check`: validates every plugin's `[package.metadata.webox]` (unique reverse-DNS `pkg`, semver version).
//! - `collect --platform <key> [--release-dir target/release] [--out dist]`: after `cargo build --release`, loads every built
//!   plugin once to read its contract and version (and checks the version matches Cargo.toml), copies it to
//!   `<out>/files/<pkg>/<version>/<platform>/<library file>` and writes `<out>/manifest-<platform>.json` with sha256 and size.
//! - `index --dist <dir> --repo <dir>`: merges every `manifest-*.json` under `<dist>`, copies the binaries into `<repo>`, removes
//!   binaries no longer listed and writes `<repo>/index.json` and `<repo>/index.min.json` (the format in Webox's docs/plugins.md).
mod meta;

use meta::PluginMeta;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

type R<T> = Result<T, String>;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("check") => meta::read_workspace().map(|m| println!("{} plugins ok", m.len())),
        Some("collect") => collect(&args[1..]),
        Some("index") => index_cmd(&args[1..]),
        _ => Err("usage: xtask check | collect --platform <key> [--release-dir <dir>] [--out <dir>] | index --dist <dir> --repo <dir>".into()),
    };
    if let Err(e) = r { eprintln!("error: {e}"); std::process::exit(1); }
}

fn opt(args: &[String], name: &str) -> Option<String> { args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned() }

pub fn sha256_file(p: &Path) -> R<(String, u64)> {
    let b = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok((Sha256::digest(&b).iter().map(|x| format!("{x:02x}")).collect(), b.len() as u64))
}

/// `windows-x86_64`, `linux-x86_64`, `macos-aarch64`: the keys Webox looks up.
pub fn host_platform() -> String { format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH) }

fn collect(args: &[String]) -> R<()> {
    let platform = opt(args, "--platform").unwrap_or_else(host_platform);
    if platform != host_platform() { return Err(format!("collect runs on the platform it collects ({} here, not {platform})", host_platform())); }
    let release = PathBuf::from(opt(args, "--release-dir").unwrap_or_else(|| "target/release".into()));
    let out = PathBuf::from(opt(args, "--out").unwrap_or_else(|| "dist".into()));
    let mut entries = vec![];
    for m in meta::read_workspace()? {
        let lib = format!("{}{}{}", std::env::consts::DLL_PREFIX, m.lib, std::env::consts::DLL_SUFFIX);
        let built = release.join(&lib);
        if !built.is_file() { return Err(format!("{} not built ({}); run cargo build --release first", m.pkg, built.display())); }
        let (contract, version) = meta::probe(&built)?;
        if version != m.version { return Err(format!("{}: the library reports version {version}, Cargo.toml says {}", m.pkg, m.version)); }
        let rel = format!("files/{}/{}/{platform}/{lib}", m.pkg, m.version);
        let dest = out.join(&rel);
        std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::copy(&built, &dest).map_err(|e| e.to_string())?;
        let (sha, size) = sha256_file(&dest)?;
        println!("{} {} contract {contract}: {rel} ({size} bytes)", m.pkg, m.version);
        entries.push(json!({ "pkg": m.pkg, "version": m.version, "contract": contract, "file": rel, "sha256": sha, "size": size }));
    }
    let manifest = json!({ "platform": platform, "plugins": entries });
    std::fs::write(out.join(format!("manifest-{platform}.json")), serde_json::to_string_pretty(&manifest).unwrap()).map_err(|e| e.to_string())
}

/// One built file of one plugin, as read from a manifest (with the folder the manifest sits in).
#[derive(Clone, Debug)]
pub struct Built { pub platform: String, pub pkg: String, pub version: String, pub contract: String, pub file: String, pub sha256: String, pub size: u64, pub dir: PathBuf }

fn read_manifests(dist: &Path) -> R<Vec<Built>> {
    let mut out = vec![];
    let mut stack = vec![dist.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?.flatten() {
            let p = e.path();
            if p.is_dir() { stack.push(p); continue; }
            let n = e.file_name().to_string_lossy().to_string();
            if !(n.starts_with("manifest-") && n.ends_with(".json")) { continue; }
            let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).map_err(|e| e.to_string())?).map_err(|e| format!("{n}: {e}"))?;
            let platform = v["platform"].as_str().ok_or(format!("{n}: no platform"))?.to_string();
            for x in v["plugins"].as_array().into_iter().flatten() {
                let s = |k: &str| x[k].as_str().map(String::from).ok_or(format!("{n}: entry without {k}"));
                out.push(Built { platform: platform.clone(), pkg: s("pkg")?, version: s("version")?, contract: s("contract")?, file: s("file")?,
                    sha256: s("sha256")?, size: x["size"].as_u64().unwrap_or(0), dir: d.clone() });
            }
        }
    }
    Ok(out)
}

/// The index document for `plugins` built as `built`. Fails when a plugin was not built at all, or when platforms disagree.
pub fn build_index(plugins: &[PluginMeta], built: &[Built]) -> R<Value> {
    let mut list = vec![];
    for m in plugins {
        let mine: Vec<&Built> = built.iter().filter(|b| b.pkg == m.pkg).collect();
        if mine.is_empty() { return Err(format!("{} was not built for any platform", m.pkg)); }
        if let Some(b) = mine.iter().find(|b| b.version != m.version) { return Err(format!("{}: {} built {} but Cargo.toml says {}", m.pkg, b.platform, b.version, m.version)); }
        let contract = &mine[0].contract;
        if let Some(b) = mine.iter().find(|b| &b.contract != contract) { return Err(format!("{}: contract {} on {} but {contract} elsewhere", m.pkg, b.contract, b.platform)); }
        let files: BTreeMap<&str, Value> = mine.iter().map(|b| (b.platform.as_str(), json!({ "file": b.file, "sha256": b.sha256, "size": b.size }))).collect();
        let mut e = json!({ "name": m.name, "pkg": m.pkg, "version": m.version, "contract": contract, "description": m.description,
            "authors": m.authors, "files": files });
        if let Some(h) = &m.homepage { e["homepage"] = json!(h); }
        if let Some(i) = &m.icon { e["icon"] = json!(format!("icons/{}.{}", m.pkg, i.extension().and_then(|x| x.to_str()).unwrap_or("png"))); }
        if let Some(w) = &m.min_webox { e["minWebox"] = json!(w); }
        list.push(e);
    }
    list.sort_by(|a, b| a["pkg"].as_str().cmp(&b["pkg"].as_str()));
    Ok(json!({
        "formatVersion": 1,
        "name": "Webox Plugins",
        "description": "Official Webox plugins, built by CI from github.com/PianoNic/WeboxPlugins",
        "homepage": "https://github.com/PianoNic/WeboxPlugins",
        "plugins": list,
    }))
}

fn index_cmd(args: &[String]) -> R<()> {
    let dist = PathBuf::from(opt(args, "--dist").ok_or("--dist missing")?);
    let repo = PathBuf::from(opt(args, "--repo").ok_or("--repo missing")?);
    let plugins = meta::read_workspace()?;
    let built = read_manifests(&dist)?;
    let idx = build_index(&plugins, &built)?;
    std::fs::create_dir_all(&repo).map_err(|e| e.to_string())?;
    let mut keep = std::collections::HashSet::new();
    for b in &built {
        let (src, dst) = (b.dir.join(&b.file), repo.join(&b.file));
        let (sha, size) = sha256_file(&src)?;
        if sha != b.sha256 || size != b.size { return Err(format!("{}: {} does not match its manifest", b.pkg, b.file)); }
        std::fs::create_dir_all(dst.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::copy(&src, &dst).map_err(|e| e.to_string())?;
        keep.insert(dst);
    }
    for m in &plugins {
        if let Some(icon) = &m.icon {
            let dst = repo.join("icons").join(format!("{}.{}", m.pkg, icon.extension().and_then(|x| x.to_str()).unwrap_or("png")));
            std::fs::create_dir_all(dst.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::copy(icon, &dst).map_err(|e| format!("{}: {e}", icon.display()))?;
            keep.insert(dst);
        }
    }
    for sub in ["files", "icons"] { prune(&repo.join(sub), &keep); }
    std::fs::write(repo.join("index.json"), serde_json::to_string_pretty(&idx).unwrap() + "\n").map_err(|e| e.to_string())?;
    std::fs::write(repo.join("index.min.json"), serde_json::to_string(&idx).unwrap()).map_err(|e| e.to_string())?;
    println!("{} plugins, {} files -> {}", plugins.len(), built.len(), repo.display());
    Ok(())
}

/// Deletes files under `dir` that are not in `keep`, then empty folders.
fn prune(dir: &Path, keep: &std::collections::HashSet<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() { prune(&p, keep); let _ = std::fs::remove_dir(&p); }
        else if !keep.contains(&p) { let _ = std::fs::remove_file(&p); }
    }
}

#[cfg(test)]
mod tests;
