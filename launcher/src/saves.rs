//! Save games and save backups (tar.gz snapshots of the whole saved/ folder).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde_json::Value;

pub struct Save {
    pub dir: PathBuf,
    pub title: String,
    pub detail: String,
    pub modified: u64,
}

pub struct Backup {
    pub path: PathBuf,
    pub name: String,
    pub size_mb: f64,
}

fn mtime(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn list(saves_dir: &Path) -> Vec<Save> {
    let Ok(entries) = fs::read_dir(saves_dir) else {
        return Vec::new();
    };
    let mut saves: Vec<Save> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .map(|dir| {
            let meta: Value = fs::read_to_string(dir.join("metadata.json"))
                .ok()
                .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
                .unwrap_or(Value::Null);
            let info = meta.get("gameinfo").unwrap_or(&meta);
            let field = |k: &str| info.get(k).and_then(Value::as_str).map(str::to_string);
            let folder = dir.file_name().unwrap_or_default().to_string_lossy().to_string();
            Save {
                title: field("town_name").or_else(|| field("name")).unwrap_or_else(|| folder.clone()),
                detail: field("formatted_time").unwrap_or(folder),
                modified: mtime(&dir),
                dir,
            }
        })
        .collect();
    saves.sort_by(|a, b| b.modified.cmp(&a.modified));
    saves
}

pub fn backup_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    base.join("rehearth/backups")
}

pub fn list_backups() -> Vec<Backup> {
    let Ok(entries) = fs::read_dir(backup_dir()) else {
        return Vec::new();
    };
    let mut backups: Vec<Backup> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(".tar.gz"))
        .map(|path| Backup {
            name: path.file_name().unwrap_or_default().to_string_lossy().trim_end_matches(".tar.gz").to_string(),
            size_mb: fs::metadata(&path).map(|m| m.len() as f64 / 1_048_576.0).unwrap_or(0.0),
            path,
        })
        .collect();
    backups.sort_by(|a, b| b.name.cmp(&a.name));
    backups
}

fn stamp() -> String {
    let out = Command::new("date").arg("+%Y-%m-%d_%H-%M-%S").output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0).to_string(),
    }
}

/// Snapshots the whole saved/ folder. Blocking; run it off the UI thread.
pub fn back_up(saves_dir: &Path) -> Result<PathBuf> {
    if !saves_dir.is_dir() {
        bail!("there are no saves yet");
    }
    let dir = backup_dir();
    fs::create_dir_all(&dir)?;
    let out = dir.join(format!("{}.tar.gz", stamp()));
    let parent = saves_dir.parent().context("saves folder has no parent")?;
    let status = Command::new("tar")
        .arg("-czf")
        .arg(&out)
        .arg("-C")
        .arg(parent)
        .arg(saves_dir.file_name().context("bad saves folder")?)
        .status()
        .context("couldn't run tar")?;
    if !status.success() {
        let _ = fs::remove_file(&out);
        bail!("tar failed");
    }
    Ok(out)
}

/// Restores a snapshot over saved/. The current saves are backed up first, so a
/// restore can always be undone.
pub fn restore(saves_dir: &Path, backup: &Path) -> Result<()> {
    if saves_dir.is_dir() {
        back_up(saves_dir).context("couldn't back up current saves before restoring")?;
        fs::remove_dir_all(saves_dir)?;
    }
    let parent = saves_dir.parent().context("saves folder has no parent")?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(backup)
        .arg("-C")
        .arg(parent)
        .status()
        .context("couldn't run tar")?;
    if !status.success() {
        bail!("tar couldn't extract {}", backup.display());
    }
    Ok(())
}
