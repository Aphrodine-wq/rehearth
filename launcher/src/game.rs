//! Where Stonehearth lives, its settings file, and starting it through Steam.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use serde_json::{Map, Value};

pub const APP_ID: &str = "253250";

pub struct GamePaths {
    pub game: PathBuf,
    pub workshop: PathBuf,
}

impl GamePaths {
    pub fn mods(&self) -> PathBuf {
        self.game.join("mods")
    }
    pub fn saves(&self) -> PathBuf {
        self.game.join("saved")
    }
    pub fn log(&self) -> PathBuf {
        self.game.join("stonehearth.log")
    }
    pub fn user_settings(&self) -> PathBuf {
        self.game.join("user_settings.json")
    }
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Where a Steam client may live: the native install, its legacy symlink, and Flatpak Steam.
pub fn steam_roots() -> Vec<PathBuf> {
    let home = home();
    [".local/share/Steam", ".steam/steam", ".steam/root", ".var/app/com.valvesoftware.Steam/.local/share/Steam"]
        .iter()
        .map(|d| home.join(d))
        .collect()
}

/// Steam library roots, from libraryfolders.vdf plus the default install.
fn steam_libraries() -> Vec<PathBuf> {
    let mut libs = Vec::new();
    for root in steam_roots() {
        let vdf = root.join("steamapps/libraryfolders.vdf");
        if let Ok(text) = fs::read_to_string(&vdf) {
            for line in text.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("\"path\"") {
                    let path = rest.trim().trim_matches('"').replace("\\\\", "\\");
                    libs.push(PathBuf::from(path));
                }
            }
        }
        libs.push(root);
    }
    libs.dedup();
    libs
}

/// Finds the install, preferring an explicit override from the launcher config.
pub fn locate(override_dir: Option<&Path>) -> Option<GamePaths> {
    let candidates: Vec<PathBuf> = match override_dir {
        Some(dir) => vec![dir.to_path_buf()],
        None => steam_libraries()
            .into_iter()
            .map(|lib| lib.join("steamapps/common/Stonehearth"))
            .collect(),
    };
    let game = candidates
        .into_iter()
        .find(|dir| dir.join("Stonehearth.exe").is_file())?;
    // workshop content sits in the same library as the game
    let library = game.parent()?.parent()?.to_path_buf();
    Some(GamePaths {
        workshop: library.join("workshop/content").join(APP_ID),
        game,
    })
}

/// "1.1.0.949" from the first line of the last session's log, if there is one.
pub fn version(paths: &GamePaths) -> Option<String> {
    let text = fs::read_to_string(paths.log()).ok()?;
    let line = text.lines().find(|l| l.contains("Stonehearth Version"))?;
    let v = line.split("Stonehearth Version").nth(1)?.trim();
    Some(v.split_whitespace().next()?.to_string())
}

/// True while any Stonehearth process is alive (matched on the Windows exe path
/// Proton passes, so this never matches the launcher itself).
pub fn is_running() -> bool {
    let Ok(entries) = fs::read_dir("/proc") else {
        return false;
    };
    entries.flatten().any(|e| {
        fs::read(e.path().join("cmdline"))
            .map(|c| String::from_utf8_lossy(&c).contains("Stonehearth\\x64\\Stonehearth.exe")
                || String::from_utf8_lossy(&c).contains("common\\Stonehearth\\Stonehearth.exe"))
            .unwrap_or(false)
    })
}

/// Starts the game through Steam so it uses the player's Proton setup.
/// `session_args` apply to this launch only (safe mode, problem-mod tests).
/// Falls back to Flatpak Steam when there's no `steam` command.
pub fn launch(extra_args: &str, session_args: &[String]) -> Result<()> {
    let run = |program: &str, prefix: &[&str]| {
        Command::new(program)
            .args(prefix)
            .arg("-applaunch")
            .arg(APP_ID)
            .args(extra_args.split_whitespace())
            .args(session_args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    };
    run("steam", &[])
        .or_else(|_| run("flatpak", &["run", "com.valvesoftware.Steam"]))
        .context("couldn't start Steam (is it installed?)")?;
    Ok(())
}

pub fn open_path(path: &Path) {
    let _ = Command::new("xdg-open")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// The game's user_settings.json. Keys are dotted paths ("renderer.enable_vsync"),
/// the same way the game reads them; unknown keys and key order are preserved.
pub struct UserSettings {
    path: PathBuf,
    root: Value,
}

impl UserSettings {
    pub fn load(paths: &GamePaths) -> Result<Self> {
        let path = paths.user_settings();
        let root = match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(text.trim_start_matches('\u{feff}'))
                .with_context(|| format!("{} isn't valid JSON", path.display()))?,
            Err(_) => Value::Object(Map::new()),
        };
        Ok(Self { path, root })
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        key.split('.').try_fold(&self.root, |v, part| v.get(part))
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(Value::as_bool)
    }

    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(Value::as_i64)
    }

    pub fn set(&mut self, key: &str, value: Value) {
        let mut node = &mut self.root;
        let mut parts = key.split('.').peekable();
        while let Some(part) = parts.next() {
            if !node.is_object() {
                *node = Value::Object(Map::new());
            }
            let map = node.as_object_mut().expect("just made it an object");
            if parts.peek().is_none() {
                map.insert(part.to_string(), value);
                return;
            }
            node = map
                .entry(part.to_string())
                .or_insert_with(|| Value::Object(Map::new()));
        }
    }

    /// Removes a key (and any parent objects it leaves empty).
    pub fn remove(&mut self, key: &str) {
        fn walk(node: &mut Value, parts: &[&str]) {
            let Some(map) = node.as_object_mut() else { return };
            if parts.len() == 1 {
                map.remove(parts[0]);
                return;
            }
            if let Some(child) = map.get_mut(parts[0]) {
                walk(child, &parts[1..]);
                if child.as_object().is_some_and(Map::is_empty) {
                    map.remove(parts[0]);
                }
            }
        }
        let parts: Vec<&str> = key.split('.').collect();
        walk(&mut self.root, &parts);
    }

    /// Writes the file, keeping a one-time backup of the player's original.
    pub fn save(&self) -> Result<()> {
        let backup = self.path.with_extension("json.rehearth-bak");
        if self.path.exists() && !backup.exists() {
            fs::copy(&self.path, &backup).context("couldn't back up user_settings.json")?;
        }
        let text = serde_json::to_string_pretty(&self.root)?;
        fs::write(&self.path, text + "\n")
            .with_context(|| format!("couldn't write {}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_keeps_other_keys_and_order() {
        let dir = std::env::temp_dir().join(format!("rehearth-settings-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let original = r#"{
	"user_id" : "abc",
	"mods" : { "base" : { "northern_alliance" : { "enabled" : false } } },
	"renderer" : { "enable_vsync" : false, "draw_distance" : 750 }
}"#;
        fs::write(dir.join("user_settings.json"), original).unwrap();
        let paths = GamePaths { game: dir.clone(), workshop: dir.join("ws") };

        let mut s = UserSettings::load(&paths).unwrap();
        assert_eq!(s.get_bool("mods.base.northern_alliance.enabled"), Some(false));
        s.set("mods.base.northern_alliance.enabled", Value::Bool(true));
        s.set("mods.directory.rehearth_patch.enabled", Value::Bool(true));
        s.save().unwrap();

        let text = fs::read_to_string(dir.join("user_settings.json")).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["user_id"], "abc");
        assert_eq!(v["renderer"]["draw_distance"], 750);
        assert_eq!(v["mods"]["base"]["northern_alliance"]["enabled"], true);
        assert_eq!(v["mods"]["directory"]["rehearth_patch"]["enabled"], true);
        // key order is preserved: user_id still comes first
        assert!(text.find("user_id").unwrap() < text.find("renderer").unwrap());
        // and the player's original is kept once
        assert_eq!(fs::read_to_string(dir.join("user_settings.json.rehearth-bak")).unwrap(), original);
        fs::remove_dir_all(dir).unwrap();
    }
}
