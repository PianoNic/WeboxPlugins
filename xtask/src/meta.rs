//! Plugin metadata from `cargo metadata` (Cargo.toml is the single source: version, description, authors, homepage, plus
//! `[package.metadata.webox]` for the display name, the reverse-DNS `pkg`, an optional `icon` file and `min-webox`), and a
//! probe that loads a built library to read what it really reports.
use crate::R;
use collabhost_plugin_api::{unpack_contract, PluginVTable, CONTRACT_SYMBOL, ENTRY_SYMBOL};
use serde_json::Value;
use std::ffi::CStr;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct PluginMeta {
    pub name: String, pub pkg: String, pub version: String, pub description: String, pub authors: Vec<String>,
    pub homepage: Option<String>, pub icon: Option<PathBuf>, pub min_webox: Option<String>,
    /// Library target name (`collabhost_slash_commands`).
    pub lib: String,
}

pub fn valid_pkg(p: &str) -> bool {
    p.len() <= 128 && p.split('.').count() >= 2
        && p.split('.').all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'))
}

pub fn read_workspace() -> R<Vec<PluginMeta>> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = std::process::Command::new(cargo).args(["metadata", "--format-version", "1", "--no-deps"]).output().map_err(|e| e.to_string())?;
    if !out.status.success() { return Err(String::from_utf8_lossy(&out.stderr).into_owned()); }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    from_metadata(&v)
}

pub fn from_metadata(v: &Value) -> R<Vec<PluginMeta>> {
    let mut list: Vec<PluginMeta> = vec![];
    for p in v["packages"].as_array().into_iter().flatten() {
        let w = &p["metadata"]["webox"];
        if w.is_null() { continue; }
        let s = |x: &Value| x.as_str().map(String::from);
        let crate_name = s(&p["name"]).unwrap_or_default();
        let pkg = s(&w["pkg"]).ok_or(format!("{crate_name}: [package.metadata.webox] needs pkg"))?;
        if !valid_pkg(&pkg) { return Err(format!("{crate_name}: pkg '{pkg}' is not a reverse-DNS id (lowercase, dots, at least two parts)")); }
        if list.iter().any(|m| m.pkg == pkg) { return Err(format!("pkg '{pkg}' is used twice")); }
        let version = s(&p["version"]).unwrap_or_default();
        semver::Version::parse(&version).map_err(|e| format!("{crate_name}: version {version}: {e}"))?;
        let lib = p["targets"].as_array().into_iter().flatten()
            .find(|t| t["crate_types"].as_array().is_some_and(|c| c.iter().any(|x| x == "cdylib")))
            .and_then(|t| s(&t["name"])).ok_or(format!("{crate_name}: needs [lib] crate-type = [\"cdylib\"]"))?.replace('-', "_");
        let dir = s(&p["manifest_path"]).map(PathBuf::from).and_then(|m| m.parent().map(Path::to_path_buf)).unwrap_or_default();
        let description = s(&p["description"]).ok_or(format!("{crate_name}: needs a description"))?;
        list.push(PluginMeta {
            name: s(&w["name"]).unwrap_or_else(|| crate_name.clone()), pkg, version, description,
            authors: p["authors"].as_array().into_iter().flatten().filter_map(|a| a.as_str()).map(|a| a.split(" <").next().unwrap_or(a).to_string()).collect(),
            homepage: s(&p["homepage"]), icon: s(&w["icon"]).map(|i| dir.join(i)), min_webox: s(&w["min-webox"]), lib,
        });
    }
    if list.is_empty() { return Err("no plugins with [package.metadata.webox] found".into()); }
    Ok(list)
}

/// Loads a built plugin and returns (contract "major.minor", version it reports). Runs the plugin's constructor once.
pub fn probe(lib: &Path) -> R<(String, String)> {
    unsafe {
        let l = libloading::Library::new(lib).map_err(|e| format!("{}: {e}", lib.display()))?;
        let contract = l.get::<extern "C" fn() -> u32>(CONTRACT_SYMBOL).map(|f| f()).unwrap_or(1 << 16);
        let (ma, mi) = unpack_contract(contract);
        let entry = l.get::<extern "C" fn() -> *const PluginVTable>(ENTRY_SYMBOL).map_err(|_| format!("{}: not a Webox plugin", lib.display()))?;
        let vt = entry();
        if vt.is_null() { return Err(format!("{}: entry returned null", lib.display())); }
        let v = &*vt;
        let version = if v.version.is_null() { String::new() } else { CStr::from_ptr(v.version).to_string_lossy().into_owned() };
        if let Some(d) = v.destroy { d(v.instance) }
        Ok((format!("{ma}.{mi}"), version))
    }
}
