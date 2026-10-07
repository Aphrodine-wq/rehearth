//! Installed mods: what's there, whether the game will load it, and installing or
//! removing the ones ReHearth manages.
//!
//! The game decides what to load from `mods.<type>.<namespace>.enabled` in
//! user_settings.json, where <type> is base (the game's own .smod files), zip (a
//! dropped-in .smod), directory (an unpacked folder) or steam_workshop.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::game::{GamePaths, UserSettings};

/// Marker file in every folder ReHearth installed; nothing without it gets deleted.
pub const MANAGED_MARKER: &str = ".rehearth-managed";

/// Mods the game can't run without; shown but never toggled.
const REQUIRED: &[&str] = &["radiant", "stonehearth"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModKind {
    Base,
    Zip,
    Directory,
    Workshop,
}

impl ModKind {
    fn config_type(self) -> &'static str {
        match self {
            ModKind::Base => "base",
            ModKind::Zip => "zip",
            ModKind::Directory => "directory",
            ModKind::Workshop => "steam_workshop",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ModKind::Base => "Built-in",
            ModKind::Zip => "Local .smod",
            ModKind::Directory => "Local folder",
            ModKind::Workshop => "Workshop",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Managed {
    pub source: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub commit: String,
    #[serde(default)]
    pub installed_at: u64,
}

#[derive(Clone, Debug)]
pub struct ModInfo {
    pub namespace: String,
    pub name: String,
    pub kind: ModKind,
    pub path: PathBuf,
    pub default_enabled: bool,
    pub debug: bool,
    pub dependencies: Vec<String>,
    pub managed: Option<Managed>,
    pub workshop_id: Option<String>,
    /// Files this mod replaces: (target path, source path), both as game
    /// resource paths like "stonehearth/ai/actions/foo.lua".
    pub overrides: Vec<(String, String)>,
    /// manifest "info.version" (the modding API version; 3 for 1.0+ mods)
    pub api_version: Option<i64>,
    /// the author's own version number, when the manifest has one
    pub mod_version: Option<String>,
    /// set when the manifest couldn't be read at all
    pub manifest_error: Option<String>,
    /// last change on disk, seconds since the epoch
    pub modified: u64,
    pub client_only: bool,
}

impl ModInfo {
    pub fn required(&self) -> bool {
        REQUIRED.contains(&self.namespace.as_str())
    }

    pub fn enabled_key(&self) -> String {
        format!("mods.{}.{}.enabled", self.kind.config_type(), self.namespace)
    }

    pub fn is_enabled(&self, settings: &UserSettings) -> bool {
        self.required() || settings.get_bool(&self.enabled_key()).unwrap_or(self.default_enabled)
    }

    pub fn set_enabled(&self, settings: &mut UserSettings, on: bool) {
        settings.set(&self.enabled_key(), Value::Bool(on));
    }
}

fn parse_manifest(text: &str) -> Result<Value, String> {
    serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("manifest.json is broken: {e}"))
}

/// manifest.json from inside an .smod (a zip holding `<namespace>/manifest.json`).
/// Some smods carry extra manifests deeper down, so take the shallowest one.
fn smod_manifest(path: &Path) -> Result<Value, String> {
    let list = Command::new("unzip").arg("-Z1").arg(path).output().map_err(|e| format!("couldn't run unzip: {e}"))?;
    if !list.status.success() {
        return Err("the .smod isn't a readable zip file".into());
    }
    let names = String::from_utf8_lossy(&list.stdout);
    let inner = names
        .lines()
        .filter(|n| n.ends_with("manifest.json") && (*n == "manifest.json" || n.ends_with("/manifest.json")))
        .min_by_key(|n| n.matches('/').count())
        .ok_or("no manifest.json inside")?
        .to_string();
    let out = Command::new("unzip").arg("-p").arg(path).arg(&inner).output().map_err(|e| e.to_string())?;
    parse_manifest(&String::from_utf8_lossy(&out.stdout))
}

/// "northern_alliance" -> "Northern Alliance", for manifests without a display name.
fn pretty(namespace: &str) -> String {
    namespace
        .split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn mtime(path: &Path) -> u64 {
    let meta = if path.is_dir() { fs::metadata(path.join("manifest.json")) } else { fs::metadata(path) };
    meta.and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// "file(ai/foo.lua)" in mod `ns` -> "ns/ai/foo.lua"; absolute paths stay as they are.
fn resource_path(ns: &str, value: &str) -> String {
    match value.strip_prefix("file(").and_then(|v| v.strip_suffix(')')) {
        Some(rel) => format!("{ns}/{}", rel.trim_start_matches("./").trim_start_matches('/')),
        None => value.trim_start_matches('/').to_string(),
    }
}

fn mod_from_manifest(manifest: &Value, fallback_ns: &str, kind: ModKind, path: PathBuf) -> ModInfo {
    let info = manifest.get("info");
    let text = |key: &str| info.and_then(|i| i.get(key)).and_then(Value::as_str).map(str::to_string);
    let namespace = text("namespace").unwrap_or_else(|| fallback_ns.to_string());
    let managed = fs::read_to_string(path.join(MANAGED_MARKER))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    ModInfo {
        name: text("name").unwrap_or_else(|| pretty(&namespace)),
        namespace,
        kind,
        default_enabled: info
            .and_then(|i| i.get("default_enabled"))
            .and_then(Value::as_bool)
            .unwrap_or(true),
        debug: info
            .and_then(|i| i.get("is_debug_mod"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        dependencies: manifest
            .get("dependencies")
            .and_then(Value::as_array)
            .map(|deps| deps.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default(),
        managed,
        workshop_id: text("steam_file_id"),
        overrides: manifest
            .get("overrides")
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .filter_map(|(target, src)| Some((target.trim_start_matches('/').to_string(), src.as_str()?.to_string())))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
            .into_iter()
            .map(|(target, src)| {
                let ns = info.and_then(|i| i.get("namespace")).and_then(Value::as_str).unwrap_or(fallback_ns);
                (target, resource_path(ns, &src))
            })
            .collect(),
        api_version: info.and_then(|i| i.get("version")).and_then(Value::as_i64),
        mod_version: info
            .and_then(|i| i.get("mod_version").or_else(|| i.get("mod_ver")))
            .map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())),
        manifest_error: None,
        modified: mtime(&path),
        client_only: info.and_then(|i| i.get("client_only")).and_then(Value::as_bool).unwrap_or(false),
        path,
    }
}

/// A mod whose manifest couldn't be read: still listed so the player sees it.
fn broken_mod(stem: &str, kind: ModKind, path: PathBuf, error: String) -> ModInfo {
    let mut m = mod_from_manifest(&Value::Null, stem, kind, path);
    m.manifest_error = Some(error);
    m.default_enabled = false;
    m
}

/// The game's own modules are the ones it checksums in stonehearth.json.
fn base_modules(game: &Path) -> Vec<String> {
    fs::read_to_string(game.join("stonehearth.json"))
        .ok()
        .and_then(|t| parse_manifest(&t).ok())
        .and_then(|v| v.get("mod_checksums").and_then(Value::as_object).cloned())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_else(|| REQUIRED.iter().map(|s| s.to_string()).collect())
}

fn scan_dir(dir: &Path, base: &[String], workshop: bool, out: &mut Vec<ModInfo>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
        if path.is_dir() {
            let manifest = path.join("manifest.json");
            if manifest.is_file() {
                let kind = if workshop { ModKind::Workshop } else { ModKind::Directory };
                let parsed = fs::read_to_string(&manifest).map_err(|e| e.to_string()).and_then(|t| parse_manifest(&t));
                let mut info = match parsed {
                    Ok(m) => mod_from_manifest(&m, &stem, kind, path),
                    Err(e) => broken_mod(&stem, kind, path, e),
                };
                if workshop && stem.parse::<u64>().is_ok() {
                    // an item unpacked straight into workshop/content/<app>/<id>/
                    info.workshop_id = Some(stem.clone());
                }
                out.push(info);
            } else if workshop {
                // workshop items are <id>/<something>.smod or <id>/<namespace>/
                let before = out.len();
                scan_dir(&path, base, true, out);
                for m in &mut out[before..] {
                    // the folder name is the item id Steam knows it by
                    m.workshop_id = Some(stem.clone());
                }
            }
        } else if path.extension().is_some_and(|e| e == "smod") {
            let kind = if workshop {
                ModKind::Workshop
            } else if base.contains(&stem) {
                ModKind::Base
            } else {
                ModKind::Zip
            };
            out.push(match smod_manifest(&path) {
                Ok(manifest) => mod_from_manifest(&manifest, &stem, kind, path),
                Err(e) => broken_mod(&stem, kind, path, e),
            });
        }
    }
}

pub fn scan(paths: &GamePaths) -> Vec<ModInfo> {
    let base = base_modules(&paths.game);
    let mut mods = Vec::new();
    scan_dir(&paths.mods(), &base, false, &mut mods);
    scan_dir(&paths.workshop, &base, true, &mut mods);
    mods
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn write_marker(dir: &Path, managed: &Managed) -> Result<()> {
    fs::write(dir.join(MANAGED_MARKER), serde_json::to_string_pretty(managed)?)?;
    Ok(())
}

/// Installs a folder mod into mods/<namespace>, replacing an older ReHearth copy.
fn place(paths: &GamePaths, staged: &Path, namespace: &str, managed: Managed) -> Result<()> {
    let dst = paths.mods().join(namespace);
    if dst.exists() {
        if !dst.join(MANAGED_MARKER).exists() {
            bail!("{} already exists and wasn't installed by ReHearth; remove it yourself first", dst.display());
        }
        fs::remove_dir_all(&dst).with_context(|| format!("couldn't remove old {}", dst.display()))?;
    }
    fs::rename(staged, &dst)
        .or_else(|_| {
            // staging can be on another filesystem; fall back to copying
            let status = Command::new("cp").arg("-a").arg(staged).arg(&dst).status()?;
            if !status.success() {
                bail!("copy failed");
            }
            fs::remove_dir_all(staged)?;
            Ok(())
        })
        .with_context(|| format!("couldn't move the mod into {}", dst.display()))?;
    write_marker(&dst, &managed)
}

fn staging_dir(paths: &GamePaths) -> PathBuf {
    // inside the game folder so the final move is a cheap rename
    paths.game.join(".rehearth-staging")
}

/// Clones a git branch and installs it as a folder mod. Blocking; run it off the UI thread.
pub fn install_git(paths: &GamePaths, url: &str, branch: &str) -> Result<String> {
    let staging = staging_dir(paths);
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;
    let checkout = staging.join("checkout");
    let status = Command::new("git")
        .args(["clone", "--quiet", "--depth", "1", "--single-branch", "--branch", branch, url])
        .arg(&checkout)
        .status()
        .context("couldn't run git")?;
    if !status.success() {
        bail!("git clone of {url} failed");
    }
    let commit = Command::new("git")
        .arg("-C")
        .arg(&checkout)
        .args(["rev-parse", "HEAD"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let _ = fs::remove_dir_all(checkout.join(".git"));

    let manifest = fs::read_to_string(checkout.join("manifest.json"))
        .ok()
        .and_then(|t| parse_manifest(&t).ok())
        .context("that repository has no readable manifest.json at its root")?;
    let namespace = manifest
        .get("info")
        .and_then(|i| i.get("namespace"))
        .and_then(Value::as_str)
        .context("manifest.json has no info.namespace")?
        .to_string();

    place(
        paths,
        &checkout,
        &namespace,
        Managed { source: url.to_string(), branch: branch.to_string(), commit, installed_at: now() },
    )?;
    let _ = fs::remove_dir_all(&staging);
    Ok(namespace)
}

/// Latest commit on a remote branch, for update checks.
pub fn remote_commit(url: &str, branch: &str) -> Option<String> {
    let out = Command::new("git").args(["ls-remote", url, branch]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace().next().map(str::to_string)
}

/// Writes a mod that ships inside the launcher binary.
pub fn install_bundled(paths: &GamePaths, namespace: &str, files: &[(&str, &str)], version: &str) -> Result<()> {
    let staging = staging_dir(paths);
    let _ = fs::remove_dir_all(&staging);
    let root = staging.join(namespace);
    for (rel, contents) in files {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, contents)?;
    }
    place(
        paths,
        &root,
        namespace,
        Managed { source: "bundled".into(), branch: String::new(), commit: version.into(), installed_at: now() },
    )?;
    let _ = fs::remove_dir_all(&staging);
    Ok(())
}

/// Deletes a mod folder, but only one ReHearth installed.
pub fn remove(m: &ModInfo) -> Result<()> {
    if m.managed.is_none() || !m.path.join(MANAGED_MARKER).exists() {
        bail!("{} wasn't installed by ReHearth, so it won't be deleted", m.name);
    }
    fs::remove_dir_all(&m.path).with_context(|| format!("couldn't delete {}", m.path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_install_scan_and_remove() {
        let game = std::env::temp_dir().join(format!("rehearth-mods-{}", std::process::id()));
        fs::create_dir_all(game.join("mods")).unwrap();
        // a mod the player put there by hand must never be touched
        fs::create_dir_all(game.join("mods/handmade")).unwrap();
        fs::write(game.join("mods/handmade/manifest.json"), r#"{"info":{"namespace":"handmade"}}"#).unwrap();
        let paths = GamePaths { game: game.clone(), workshop: game.join("workshop") };

        let files = [("manifest.json", r#"{"info":{"name":"Test","namespace":"rehearth_patch"},"dependencies":["stonehearth_ace"]}"#)];
        install_bundled(&paths, "rehearth_patch", &files, "9.9.9").unwrap();
        // reinstalling over our own copy is fine
        install_bundled(&paths, "rehearth_patch", &files, "9.9.9").unwrap();

        let mods = scan(&paths);
        let patch = mods.iter().find(|m| m.namespace == "rehearth_patch").unwrap();
        assert_eq!(patch.kind, ModKind::Directory);
        assert_eq!(patch.dependencies, vec!["stonehearth_ace"]);
        assert_eq!(patch.managed.as_ref().unwrap().commit, "9.9.9");
        let handmade = mods.iter().find(|m| m.namespace == "handmade").unwrap();
        assert_eq!(handmade.name, "Handmade");

        assert!(remove(handmade).is_err());
        assert!(game.join("mods/handmade").exists());
        remove(patch).unwrap();
        assert!(!game.join("mods/rehearth_patch").exists());
        assert!(!game.join(".rehearth-staging").exists());
        fs::remove_dir_all(game).unwrap();
    }
}
