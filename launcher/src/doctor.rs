//! Reads stonehearth.log from the last session and turns it into something a
//! player can act on: what broke, which mod it came from, what that kind of
//! error usually means, whether the game crashed, and what it was doing last.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ErrorKind {
    /// a Lua script error with a traceback
    Script,
    /// a JavaScript error in the game's UI
    Ui,
    /// a file, manifest or mod-loading problem
    Loading,
}

impl ErrorKind {
    pub fn label(self) -> &'static str {
        match self {
            ErrorKind::Script => "Script error",
            ErrorKind::Ui => "UI error",
            ErrorKind::Loading => "Loading problem",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScriptError {
    pub kind: ErrorKind,
    pub message: String,
    pub traceback: Vec<String>,
    /// mods named in the traceback (game's own mods excluded), innermost first
    pub mods: Vec<String>,
    /// "stonehearth_ace/ai/foo.lua:123" — where it went wrong, when known
    pub location: Option<String>,
    /// the innermost traceback line in a non-game mod, when there is one
    pub mod_location: Option<String>,
    pub count: usize,
    /// time of day it first happened, "21:17:11"
    pub first_seen: String,
    /// "server" (game logic) or "client" (UI and rendering)
    pub side: String,
}

impl ScriptError {
    /// The file part of `location`, e.g. "stonehearth/ai/foo.lua".
    pub fn file(&self) -> Option<&str> {
        self.location.as_deref().map(|l| l.split(':').next().unwrap_or(l))
    }
}

#[derive(Default)]
pub struct Report {
    pub found: bool,
    pub version: Option<String>,
    pub started: Option<String>,
    pub ended: Option<String>,
    pub clean_exit: bool,
    pub patch_loaded: Option<String>,
    pub duplicate_think_outputs: usize,
    pub errors: Vec<ScriptError>,
    /// the warnings repeated most often: (category, message, count)
    pub noisy: Vec<(String, String, usize)>,
    /// the last lines of the log, for when the game didn't close normally
    pub tail: Vec<String>,
    pub gpu: Option<String>,
    pub lines: usize,
}

/// Mods that ship with the game; a traceback through them doesn't blame anyone.
pub const GAME_MODS: &[&str] = &["radiant", "stonehearth", "northern_alliance", "rayyas_children", "debugtools"];

struct Line<'a> {
    time: &'a str,
    side: &'a str,
    level: &'a str,
    category: &'a str,
    message: &'a str,
}

/// "2026-10-04 21:17:11.160171 |  server |  1 |   app | text"
fn split(line: &str) -> Option<Line<'_>> {
    let mut parts = line.splitn(5, '|');
    let stamp = parts.next()?.trim();
    let side = parts.next()?.trim();
    let level = parts.next()?.trim();
    let category = parts.next()?.trim();
    let message = parts.next()?.trim();
    Some(Line { time: stamp.get(11..19).unwrap_or(stamp), side, level, category, message })
}

fn message_of(line: &str) -> &str {
    split(line).map(|l| l.message).unwrap_or(line.trim())
}

/// "stonehearth_ace/ai/foo.lua:12: in function ..." -> ("stonehearth_ace/ai/foo.lua:12", "stonehearth_ace")
fn frame_location(frame: &str) -> Option<(String, String)> {
    let frame = frame.trim();
    let mut parts = frame.splitn(3, ':');
    let path = parts.next()?;
    let line = parts.next().filter(|l| l.chars().all(|c| c.is_ascii_digit()) && !l.is_empty());
    if !(path.ends_with(".lua") || path.ends_with(".js")) || !path.contains('/') {
        return None;
    }
    let ns = path.trim_start_matches('/').split('/').next()?.to_string();
    let loc = match line {
        Some(l) => format!("{}:{l}", path.trim_start_matches('/')),
        None => path.trim_start_matches('/').to_string(),
    };
    Some((loc, ns))
}

/// Groups messages that differ only by numbers (entity ids, counts, line numbers).
fn normalize(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut in_digits = false;
    for c in msg.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                out.push('#');
            }
            in_digits = true;
            continue;
        }
        in_digits = false;
        out.push(c);
    }
    out
}

const LOADING_PATTERNS: &[&str] = &[
    "failed to load",
    "Error reading manifest",
    "Error loading manifest",
    "error looking for manifest",
    "could not find",
    "Could not find",
    "Circular dependency",
    "disabled due to circular dependency",
    "is missing the \"namespace\"",
    "Mixin invalid format",
    "infinite loop",
    "no manifest",
    "Mod Discovery failed",
];

pub fn read(log: &Path) -> Report {
    let Ok(bytes) = fs::read(log) else {
        return Report::default();
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut report = Report { found: true, ..Default::default() };
    let mut errors: Vec<ScriptError> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut noise: HashMap<(String, String), usize> = HashMap::new();
    let mut add = |errors: &mut Vec<ScriptError>, e: ScriptError| {
        let key = error_key(&e);
        match index.get(&key) {
            Some(&i) => errors[i].count += 1,
            None => {
                index.insert(key, errors.len());
                errors.push(e);
            }
        }
    };

    let lines: Vec<&str> = text.lines().collect();
    report.lines = lines.len();
    report.started = lines.first().and_then(|l| l.get(..19)).map(str::to_string);
    report.ended = lines.iter().rev().find_map(|l| l.get(..19).filter(|s| s.starts_with("20"))).map(str::to_string);
    report.tail = lines.iter().rev().take(40).rev().map(|l| l.to_string()).collect();

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let Some(l) = split(line) else {
            i += 1;
            continue;
        };
        let msg = l.message;
        if let Some(v) = msg.strip_prefix("Stonehearth Version ") {
            report.version.get_or_insert_with(|| v.to_string());
        } else if msg.contains("duplicate \"set_think_output\"") {
            report.duplicate_think_outputs += 1;
        } else if msg.starts_with("ReHearth Patch ") && msg.ends_with(" loaded") {
            report.patch_loaded = Some(msg.trim_end_matches(" loaded").trim_start_matches("ReHearth Patch ").to_string());
        } else if msg.contains("exiting process") {
            report.clean_exit = true;
        } else if msg.starts_with("Initializing GL2 backend") {
            report.gpu = msg.split(" on '").nth(1).map(|g| g.trim_end_matches(['\'', '.']).to_string());
        } else if msg.starts_with("-- ") && msg.contains("Error") && !msg.contains("Error End") {
            // the message is the next line; the traceback follows until "Lua Error End"
            let (time, side) = (l.time.to_string(), l.side.to_string());
            let mut message = String::new();
            let mut traceback = Vec::new();
            i += 1;
            while i < lines.len() && !message_of(lines[i]).contains("Error End") {
                let m = message_of(lines[i]);
                if message.is_empty() {
                    message = m.to_string();
                } else if m != "stack traceback:" {
                    traceback.push(m.to_string());
                }
                i += 1;
            }
            let mut mods: Vec<String> = Vec::new();
            let mut location = frame_location(&message).map(|(loc, _)| loc);
            let mut mod_location = None;
            for frame in std::iter::once(&message).chain(traceback.iter()) {
                if let Some((loc, ns)) = frame_location(frame) {
                    location.get_or_insert(loc.clone());
                    if !GAME_MODS.contains(&ns.as_str()) {
                        mod_location.get_or_insert(loc);
                        if !mods.contains(&ns) {
                            mods.push(ns);
                        }
                    }
                }
            }
            add(&mut errors, ScriptError { kind: ErrorKind::Script, message, traceback, mods, location, mod_location, count: 1, first_seen: time, side });
        } else if (l.category == "browser" || l.category.starts_with("ui")) && (msg.contains("Uncaught") || msg.contains("Error:")) {
            let location = msg
                .split_whitespace()
                .find(|w| w.contains(".js"))
                .and_then(|w| frame_location(w.trim_start_matches("http://radiant/").trim_matches(['(', ')', ','])))
                .map(|(loc, _)| loc);
            let mods = location
                .as_deref()
                .and_then(|loc| loc.split('/').next())
                .filter(|ns| !GAME_MODS.contains(ns))
                .map(|ns| vec![ns.to_string()])
                .unwrap_or_default();
            add(&mut errors, ScriptError {
                kind: ErrorKind::Ui, message: msg.to_string(), traceback: Vec::new(), mods, mod_location: None, location,
                count: 1, first_seen: l.time.to_string(), side: l.side.to_string(),
            });
        } else if l.level <= "1" && LOADING_PATTERNS.iter().any(|p| msg.contains(p)) {
            let mods = msg
                .split(['"', '\'', ' '])
                .filter(|w| w.contains('/') || w.ends_with(".smod"))
                .filter_map(|w| w.trim_start_matches('/').split('/').next())
                .map(|ns| ns.trim_end_matches(".smod").to_string())
                .filter(|ns| !ns.is_empty() && !GAME_MODS.contains(&ns.as_str()))
                .take(1)
                .collect();
            add(&mut errors, ScriptError {
                kind: ErrorKind::Loading, message: msg.to_string(), traceback: Vec::new(), mods, location: None, mod_location: None,
                count: 1, first_seen: l.time.to_string(), side: l.side.to_string(),
            });
        } else if l.level == "1" && !msg.is_empty() {
            *noise.entry((l.category.to_string(), normalize(msg))).or_default() += 1;
        }
        i += 1;
    }

    errors.sort_by(|a, b| b.count.cmp(&a.count));
    report.errors = errors;
    let mut noisy: Vec<(String, String, usize)> = noise.into_iter().filter(|(_, n)| *n >= 20).map(|((c, m), n)| (c, m, n)).collect();
    noisy.sort_by(|a, b| b.2.cmp(&a.2));
    noisy.truncate(6);
    report.noisy = noisy;
    report
}

/// Identifies "the same error" across sessions.
pub fn error_key(e: &ScriptError) -> String {
    format!("{:?}:{}", e.kind, normalize(&e.message))
}

/// The last `n` lines of the log, read from its end so huge logs stay cheap.
pub fn tail(log: &Path, n: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = fs::File::open(log) else { return Vec::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(96 * 1024);
    let mut buf = Vec::new();
    if f.seek(SeekFrom::Start(start)).is_err() || f.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().skip(usize::from(start > 0)).collect();
    lines[lines.len().saturating_sub(n)..].iter().map(|l| l.to_string()).collect()
}

/// What this kind of error usually means, in plain words.
pub fn explain(e: &ScriptError) -> &'static str {
    let m = e.message.as_str();
    let has = |s: &str| m.contains(s);
    if e.kind == ErrorKind::Loading {
        if has("ircular") {
            return "Two or more mods list each other as dependencies, so the game switched them off. The Mods screen can pick which one to keep.";
        }
        if has("manifest") || has("namespace") {
            return "A mod's manifest.json is broken or incomplete, so the game couldn't load it. Update or remove that mod.";
        }
        if has("infinite loop") || has("Mixin") {
            return "A mod's data file merges into itself or is malformed. That mod needs an update from its author.";
        }
        return "The game couldn't find or load a file. Usually a mod is missing something it depends on, or two mods disagree about a file.";
    }
    if e.kind == ErrorKind::Ui {
        return "Something in the game's interface broke. Usually a UI mod made for a different ACE or game version; the game keeps running but a window may not work.";
    }
    if has("attempt to index") && has("nil value") {
        "The code looked something up that doesn't exist. Most often a mod written for a different version of ACE or Stonehearth, or one missing a dependency."
    } else if has("attempt to call") {
        "The code called a function that isn't there. Classic sign of two mods built for different versions of each other."
    } else if has("attempt to concatenate") || has("attempt to perform arithmetic") || has("attempt to compare") {
        "Unexpected data reached the code (often an empty value). Usually harmless one-offs; many repeats point at a mod clash."
    } else if has("stack overflow") {
        "Code kept calling itself until it ran out of room. A bug or a loop between two mods; expect lag before it."
    } else if has("not enough memory") || has("out of memory") {
        "The game ran out of memory. Big towns plus many mods can do this; fewer mods or a fresh save helps."
    } else if has("json") || has("JSON") {
        "A data file couldn't be read. A mod shipped a broken JSON file."
    } else if has("unknown") && has("uri") {
        "Something referred to an item or file that doesn't exist. Usually a missing mod or a save made with mods that are now off."
    } else {
        "A script failed. If it repeats a lot, the mod named here is the first place to look."
    }
}

/// Plain-text version for pasting into Discord or a bug report.
pub fn to_text(r: &Report) -> String {
    let mut out = String::new();
    out += &format!("Stonehearth {} · session {} → {}\n",
        r.version.as_deref().unwrap_or("?"), r.started.as_deref().unwrap_or("?"), r.ended.as_deref().unwrap_or("?"));
    if let Some(gpu) = &r.gpu {
        out += &format!("GPU: {gpu}\n");
    }
    out += &format!("Ended cleanly: {}\n", if r.clean_exit { "yes" } else { "no" });
    out += &format!("ReHearth Patch: {}\n", r.patch_loaded.as_deref().unwrap_or("not loaded"));
    out += &format!("Duplicate item-search warnings: {}\n", r.duplicate_think_outputs);
    out += &format!("Errors: {}\n", r.errors.len());
    for e in &r.errors {
        let who = if e.mods.is_empty() { "game".to_string() } else { e.mods.join(", ") };
        out += &format!("\n[{}x] {} ({who}) first at {}\n  {}\n", e.count, e.kind.label(), e.first_seen, e.message);
        for t in e.traceback.iter().take(12) {
            out += &format!("    {t}\n");
        }
    }
    if !r.noisy.is_empty() {
        out += "\nMost repeated warnings:\n";
        for (cat, msg, n) in &r.noisy {
            out += &format!("  {n}x [{cat}] {msg}\n");
        }
    }
    if !r.clean_exit {
        out += "\nLast lines of the log:\n";
        for l in r.tail.iter().rev().take(15).rev() {
            out += &format!("  {l}\n");
        }
    }
    out
}

/// Plain-text version of a single error.
pub fn error_text(e: &ScriptError) -> String {
    let mut out = format!("{} ×{} ({})\n{}\n", e.kind.label(), e.count, e.side, e.message);
    for t in &e.traceback {
        out += &format!("  {t}\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_script_error() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/script_error.log");
        let r = read(&path);
        assert!(r.found);
        assert_eq!(r.version.as_deref(), Some("1.1.0.949 (x64)"));
        assert_eq!(r.errors.len(), 1);
        let e = &r.errors[0];
        assert!(e.message.contains("attempt to concatenate local 'modname'"), "{}", e.message);
        // radiant's own frames don't count as the culprit; the mod that called in does
        assert_eq!(e.mods, vec!["rehearth_bench".to_string()]);
        assert_eq!(e.location.as_deref(), Some("radiant/lib/util.lua:234"));
        assert_eq!(e.mod_location.as_deref(), Some("rehearth_bench/worlds/big_town_world.lua:56"));
        assert!(e.traceback.iter().any(|t| t.contains("big_town_world.lua:56")));
        assert_eq!(e.first_seen, "23:01:46");
    }

    #[test]
    fn counts_patch_spam_loading_problems_and_noise() {
        let dir = std::env::temp_dir().join(format!("rehearth-doctor-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("stonehearth.log");
        let mut text = String::from("\
2026-10-04 21:17:11.160171 |       server |  1 |                              app | Stonehearth Version 1.1.0.949 (x64)
2026-10-04 21:17:12.000000 |       server |  0 |               mod rehearth_patch | ReHearth Patch 0.1.0 loaded
2026-10-04 21:17:12.100000 |       server |  1 |                        resources | failed to load smod \"broken_mod.smod\"
2026-10-04 21:20:00.000000 |       server |  1 |                ai.execution_unit | (8039 Petra) [find reachable entity type anywhere] duplicate \"set_think_output\" in state ready
2026-10-04 21:20:01.000000 |       server |  1 |                ai.execution_unit | (8039 Petra) [find reachable entity type anywhere] duplicate \"set_think_output\" in state ready
");
        for n in 0..25 {
            text += &format!("2026-10-04 21:21:{:02}.000000 |       server |  1 |  mod stonehearth.inventory | Restock director stuck {n} times\n", n % 60);
        }
        text += "2026-10-04 23:21:33.040892 |       client |  0 |                      game_engine | window closed.  exiting process\n";
        fs::write(&log, text).unwrap();
        let r = read(&log);
        assert_eq!(r.patch_loaded.as_deref(), Some("0.1.0"));
        assert_eq!(r.duplicate_think_outputs, 2);
        assert!(r.clean_exit);
        assert_eq!(r.errors.len(), 1);
        assert_eq!(r.errors[0].kind, ErrorKind::Loading);
        assert_eq!(r.errors[0].mods, vec!["broken_mod".to_string()]);
        assert_eq!(r.noisy[0].2, 25, "numbers are folded so repeats group: {:?}", r.noisy);
        assert_eq!(r.ended.as_deref(), Some("2026-10-04 23:21:33"));
        fs::remove_dir_all(dir).unwrap();
    }
}
