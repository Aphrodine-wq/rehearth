//! Keeping ReHearth itself up to date from the project's GitHub releases.
//!
//! A release copy (the AppImage, or the binary `install.sh` puts in ~/.local/bin)
//! replaces itself in place. A copy built from source only reports that a release
//! exists, so an update can never overwrite a build someone is working on.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;

pub const REPO: &str = "Aphrodine-wq/rehearth";

/// How this copy was installed, which decides what an update replaces.
pub enum Install {
    /// running from an AppImage: replace that file
    AppImage(PathBuf),
    /// a release binary: replace it
    Binary(PathBuf),
    /// built with cargo: leave it alone
    Source,
    /// installed by the system package manager (e.g. the AUR): it updates it
    Package,
}

impl Install {
    pub fn detect() -> Self {
        if let Some(path) = std::env::var_os("APPIMAGE") {
            return Install::AppImage(PathBuf::from(path));
        }
        let Ok(exe) = std::env::current_exe() else {
            return Install::Source;
        };
        let s = exe.to_string_lossy();
        if s.starts_with("/usr/") || s.starts_with("/opt/") {
            Install::Package
        } else if s.contains("/.cargo/bin/") || s.contains("/target/") {
            Install::Source
        } else {
            Install::Binary(exe)
        }
    }

    fn target(&self) -> Option<(&Path, &'static str)> {
        match self {
            Install::AppImage(p) => Some((p, "ReHearth-x86_64.AppImage")),
            Install::Binary(p) => Some((p, "rehearth-linux-x86_64")),
            Install::Source | Install::Package => None,
        }
    }

    pub fn can_replace(&self) -> bool {
        self.target().is_some()
    }

    /// How to update a copy that can't replace itself.
    pub fn manual_hint(&self) -> &'static str {
        match self {
            Install::Package => "update it with your package manager",
            _ => "git pull to update",
        }
    }
}

pub fn current() -> &'static str {
    // debug builds can pretend to be older, to try the update path
    #[cfg(debug_assertions)]
    if let Ok(v) = std::env::var("REHEARTH_PRETEND_VERSION") {
        return Box::leak(v.into_boxed_str());
    }
    env!("CARGO_PKG_VERSION")
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .user_agent(concat!("ReHearth/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn parse(v: &str) -> Vec<u64> {
    v.trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// True when `latest` is a later version than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    parse(latest) > parse(current)
}

/// The newest release's version, if it is newer than this copy.
pub fn check() -> Result<Option<String>> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let text = agent()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .context("couldn't reach GitHub")?
        .body_mut()
        .read_to_string()?;
    let json: Value = serde_json::from_str(&text)?;
    let tag = json["tag_name"].as_str().context("no release found")?;
    let version = tag.trim_start_matches('v').to_string();
    Ok(is_newer(&version, current()).then_some(version))
}

/// Downloads `version` and puts it in place of this copy. The running program
/// keeps working; the new one starts next time.
pub fn install(version: &str) -> Result<PathBuf> {
    let how = Install::detect();
    let Some((target, asset)) = how.target() else {
        match how {
            Install::Package => bail!("this copy came from your package manager; update it there"),
            _ => bail!("this copy was built from source; update it with `git pull` and `cargo install --path launcher`"),
        }
    };
    let url = format!("https://github.com/{REPO}/releases/download/v{version}/{asset}");
    let dir = target.parent().context("odd install path")?;
    let partial = dir.join(format!(".{asset}.download"));
    let result = (|| -> Result<()> {
        let response = agent().get(&url).call().with_context(|| format!("couldn't download {url}"))?;
        let mut file = fs::File::create(&partial)
            .with_context(|| format!("can't write to {} (installed by a package manager? update it there)", dir.display()))?;
        io::copy(&mut response.into_body().into_reader(), &mut file)?;
        file.flush()?;
        drop(file);
        let mut head = [0u8; 4];
        io::Read::read_exact(&mut fs::File::open(&partial)?, &mut head)?;
        if &head != b"\x7fELF" {
            bail!("the download isn't a Linux program");
        }
        fs::set_permissions(&partial, fs::Permissions::from_mode(0o755))?;
        // a rename swaps the file even while this copy is running from it
        fs::rename(&partial, target)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result?;
    Ok(target.to_path_buf())
}

/// Starts the freshly installed copy; the caller exits right after.
pub fn restart(path: &Path) {
    let _ = std::process::Command::new(path).spawn();
}

/// `rehearth --update`: check and install from the command line.
pub fn run_cli() -> i32 {
    match check() {
        Ok(None) => {
            println!("ReHearth {} is the latest.", current());
            0
        }
        Ok(Some(v)) => {
            println!("Updating ReHearth {} → {v}…", current());
            match install(&v) {
                Ok(path) => {
                    println!("Done: {}", path.display());
                    0
                }
                Err(e) => {
                    eprintln!("{e:#}");
                    1
                }
            }
        }
        Err(e) => {
            eprintln!("{e:#}");
            1
        }
    }
}

/// Puts ReHearth in the desktop's app menu, pointing at this copy. Used by the
/// AppImage, which has no installer of its own.
pub fn add_to_app_menu() -> Result<PathBuf> {
    let exe = match Install::detect() {
        Install::AppImage(p) | Install::Binary(p) => p,
        Install::Source | Install::Package => std::env::current_exe()?,
    };
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    let icon_dir = data.join("icons/hicolor/256x256/apps");
    fs::create_dir_all(&icon_dir)?;
    fs::write(icon_dir.join("rehearth.png"), ICON_PNG)?;
    let apps = data.join("applications");
    fs::create_dir_all(&apps)?;
    let entry = apps.join("rehearth.desktop");
    fs::write(&entry, DESKTOP.replace("Exec=rehearth", &format!("Exec=\"{}\"", exe.display())))?;
    Ok(entry)
}

/// True when some menu entry for ReHearth exists, the user's own or a package's.
pub fn in_app_menu() -> bool {
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    std::iter::once(home)
        .chain(system.split(':').filter(|d| !d.is_empty()).map(PathBuf::from))
        .any(|dir| dir.join("applications/rehearth.desktop").is_file())
}

pub const ICON_PNG: &[u8] = include_bytes!("../../packaging/rehearth.png");
const DESKTOP: &str = include_str!("../../packaging/rehearth.desktop");

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.99.1"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
    }
}
