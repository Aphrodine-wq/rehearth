//! Works out how Stonehearth will load the enabled mods, what will go wrong,
//! and how to fix it.
//!
//! How the game does it (from the official modding guide and the engine):
//! base mods load first; every other mod loads after the mods listed in its
//! manifest "dependencies". A dependency that's missing or switched off is
//! silently ignored, mods in a dependency loop are switched off for the
//! session, and when two mods replace ("override") the same file, the one
//! that loads last wins. Two mods with no dependency between them have no
//! guaranteed order, so which file wins is luck.
//!
//! The fix for that last case is a tiny generated mod, ReHearth Load Order,
//! that depends on every mod in a conflict (so it always loads after them)
//! and overrides each contested file with the copy we chose. Nothing is
//! copied: overrides can point straight at another mod's file.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::game::{GamePaths, UserSettings};
use crate::mods::{self, ModInfo, ModKind};

pub const LOAD_ORDER_NS: &str = "rehearth_load_order";
const PATCH_NS: &str = "rehearth_patch";

/// Manifest dependencies that are only load-order hints, never requirements.
pub const SOFT_DEPENDENCIES: &[&str] = &["metaclass_override"];

/// Mods we know where to get, for "missing dependency" fixes.
pub const KNOWN_WORKSHOP: &[(&str, u64, &str)] = &[("stonehearth_ace", 1577375188, "ACE")];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    Problem,
    Warning,
    Note,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fix {
    Enable(Vec<usize>),
    Disable(Vec<usize>),
    InstallWorkshop(u64, String),
    /// (re)write or remove the generated load-order mod
    WriteLoadOrder,
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub level: Level,
    pub title: String,
    pub detail: String,
    pub fix: Option<(String, Fix)>,
    /// safe to apply without asking (the "auto-fix before launch" set)
    pub automatic: bool,
}

#[derive(Clone, Debug)]
pub struct Conflict {
    /// the file both mods replace, e.g. "stonehearth/ai/actions/foo.lua"
    pub target: String,
    /// every enabled mod that replaces it, in load order (indices into the mod list)
    pub mods: Vec<usize>,
    /// the mod whose file the game ends up using (after our fix, when unsettled)
    pub winner: usize,
    /// true when dependencies already decide the winner
    pub settled: bool,
}

#[derive(Default)]
pub struct Analysis {
    /// enabled mods in the order the game will load them
    pub order: Vec<usize>,
    pub issues: Vec<Issue>,
    pub conflicts: Vec<Conflict>,
    /// what ReHearth Load Order should contain right now (target -> source)
    pub wanted_overrides: BTreeMap<String, String>,
    pub wanted_dependencies: BTreeSet<String>,
}

impl Analysis {
    pub fn problems(&self) -> usize {
        self.issues.iter().filter(|i| i.level == Level::Problem).count()
    }

    pub fn automatic_fixes(&self) -> Vec<(String, Fix)> {
        self.issues.iter().filter(|i| i.automatic).filter_map(|i| i.fix.clone()).collect()
    }
}

fn names(mods: &[ModInfo], idx: &[usize]) -> String {
    let list: Vec<&str> = idx.iter().map(|&i| mods[i].name.as_str()).collect();
    match list.len() {
        0 => String::new(),
        1 => list[0].to_string(),
        n => format!("{} and {}", list[..n - 1].join(", "), list[n - 1]),
    }
}

/// Picks which copy of a mod to keep when it's installed more than once:
/// the most recently changed, preferring the Workshop copy on a tie.
fn best_copy(mods: &[ModInfo], copies: &[usize]) -> usize {
    *copies
        .iter()
        .max_by_key(|&&i| (mods[i].modified, mods[i].kind == ModKind::Workshop, std::cmp::Reverse(i)))
        .expect("at least one copy")
}

pub fn analyze(
    mods: &[ModInfo],
    settings: &UserSettings,
    pins: &BTreeMap<String, String>,
    current_load_order: Option<&Value>,
) -> Analysis {
    let mut a = Analysis::default();
    let on = |i: usize| mods[i].is_enabled(settings);
    let ours = |m: &ModInfo| m.namespace == LOAD_ORDER_NS;

    // copies of each namespace, and which are switched on
    let mut by_ns: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, m) in mods.iter().enumerate() {
        by_ns.entry(m.namespace.as_str()).or_default().push(i);
    }

    // broken manifests the game would choke on
    for (i, m) in mods.iter().enumerate() {
        if let Some(err) = &m.manifest_error {
            if on(i) {
                a.issues.push(Issue {
                    level: Level::Problem,
                    title: format!("{} can't be read", m.name),
                    detail: format!("{err}. The game can't load it; switching it off avoids errors at startup."),
                    fix: Some(("Switch it off".into(), Fix::Disable(vec![i]))),
                    automatic: true,
                });
            }
        }
    }

    // the same mod switched on twice (e.g. Workshop copy plus a local folder)
    let mut enabled: Vec<usize> = Vec::new();
    let mut ns_list: Vec<&&str> = by_ns.keys().collect();
    ns_list.sort();
    for ns in ns_list {
        let copies: Vec<usize> = by_ns[*ns].iter().copied().filter(|&i| on(i) && mods[i].manifest_error.is_none()).collect();
        if copies.is_empty() || ours(&mods[copies[0]]) {
            continue;
        }
        let keep = best_copy(mods, &copies);
        if copies.len() > 1 {
            let drop: Vec<usize> = copies.iter().copied().filter(|&i| i != keep).collect();
            a.issues.push(Issue {
                level: Level::Problem,
                title: format!("{} is installed {} times", mods[keep].name, copies.len()),
                detail: format!(
                    "Only one copy should be on. Keeping the newest ({}), switching off the {}.",
                    mods[keep].kind.label(),
                    drop.iter().map(|&i| mods[i].kind.label()).collect::<Vec<_>>().join(" and ")
                ),
                fix: Some(("Keep the newest".into(), Fix::Disable(drop))),
                automatic: true,
            });
        }
        enabled.push(keep);
    }
    let enabled_ns: HashSet<&str> = enabled.iter().map(|&i| mods[i].namespace.as_str()).collect();

    // requirements: installed but switched off, or not installed at all
    let mut want_on: BTreeMap<usize, Vec<usize>> = BTreeMap::new(); // dep -> who needs it
    let mut missing: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for &i in &enabled {
        for dep in &mods[i].dependencies {
            if enabled_ns.contains(dep.as_str()) || SOFT_DEPENDENCIES.contains(&dep.as_str()) {
                continue;
            }
            match by_ns.get(dep.as_str()) {
                Some(copies) => {
                    let best = best_copy(mods, copies);
                    // debug-only mods (like Debug Tools) are load-order hints, not requirements
                    if !mods[best].debug && mods[best].manifest_error.is_none() {
                        want_on.entry(best).or_default().push(i);
                    }
                }
                None => missing.entry(dep.as_str()).or_default().push(i),
            }
        }
    }
    if !want_on.is_empty() {
        let deps: Vec<usize> = want_on.keys().copied().collect();
        let mut needers: Vec<usize> = want_on.values().flatten().copied().collect();
        needers.sort();
        needers.dedup();
        a.issues.push(Issue {
            level: Level::Problem,
            title: format!("{} {} switched off", names(mods, &deps), if deps.len() == 1 { "is" } else { "are" }),
            detail: format!("{} {} on {}. Without {} it throws errors or misses content.",
                names(mods, &needers), if needers.len() == 1 { "builds" } else { "build" }, names(mods, &deps),
                if deps.len() == 1 { "it" } else { "them" }),
            fix: Some(("Switch on".into(), Fix::Enable(deps))),
            automatic: true,
        });
    }
    for (dep, needers) in &missing {
        let known = KNOWN_WORKSHOP.iter().find(|(ns, ..)| ns == dep);
        a.issues.push(Issue {
            level: if known.is_some() { Level::Problem } else { Level::Warning },
            title: format!("{} needs {}, which isn't installed", names(mods, needers), known.map(|k| k.2).unwrap_or(dep)),
            detail: "The game loads it anyway, but anything it builds on will be missing and it may throw errors.".into(),
            fix: known.map(|(_, id, label)| (format!("Install {label}"), Fix::InstallWorkshop(*id, label.to_string()))),
            automatic: false,
        });
    }
    for &i in &enabled {
        let m = &mods[i];
        if m.kind != ModKind::Base && m.api_version.is_some_and(|v| v < 3) {
            a.issues.push(Issue {
                level: Level::Warning,
                title: format!("{} was made for an old Stonehearth", m.name),
                detail: format!("Its manifest says modding version {}; 1.0 and later use 3. It may not work.", m.api_version.unwrap_or(0)),
                fix: Some(("Switch it off".into(), Fix::Disable(vec![i]))),
                automatic: false,
            });
        }
    }

    // load order: base mods first (radiant, stonehearth, then the rest), then
    // everything else after its dependencies, ties broken alphabetically
    let pos_of: HashMap<&str, usize> = enabled.iter().map(|&i| (mods[i].namespace.as_str(), i)).collect();
    let deps_of = |i: usize| -> Vec<usize> {
        mods[i].dependencies.iter().filter_map(|d| pos_of.get(d.as_str()).copied()).filter(|&d| d != i).collect()
    };
    let core_rank = |m: &ModInfo| match m.namespace.as_str() {
        "radiant" => 0,
        "stonehearth" => 1,
        _ => 2,
    };
    let mut base: Vec<usize> = enabled.iter().copied().filter(|&i| mods[i].kind == ModKind::Base).collect();
    base.sort_by(|&x, &y| core_rank(&mods[x]).cmp(&core_rank(&mods[y])).then(mods[x].namespace.cmp(&mods[y].namespace)));
    let rest: Vec<usize> = enabled.iter().copied().filter(|&i| mods[i].kind != ModKind::Base).collect();
    let mut indegree: HashMap<usize, usize> = rest.iter().map(|&i| (i, 0)).collect();
    let mut dependents: HashMap<usize, Vec<usize>> = HashMap::new();
    for &i in &rest {
        for d in deps_of(i) {
            if indegree.contains_key(&d) {
                *indegree.get_mut(&i).expect("rest") += 1;
                dependents.entry(d).or_default().push(i);
            }
        }
    }
    let mut ready: BTreeSet<(String, usize)> =
        indegree.iter().filter(|(_, n)| **n == 0).map(|(&i, _)| (mods[i].namespace.clone(), i)).collect();
    a.order = base;
    while let Some(next) = ready.pop_first() {
        let i = next.1;
        a.order.push(i);
        for &d in dependents.get(&i).map(Vec::as_slice).unwrap_or(&[]) {
            let n = indegree.get_mut(&d).expect("dependent");
            *n -= 1;
            if *n == 0 {
                ready.insert((mods[d].namespace.clone(), d));
            }
        }
    }
    let looped: Vec<usize> = rest.iter().copied().filter(|i| !a.order.contains(i)).collect();
    if !looped.is_empty() {
        // keep the mod others lean on most, switch off the rest of the loop
        let keep = *looped
            .iter()
            .max_by_key(|&&i| dependents.get(&i).map(Vec::len).unwrap_or(0))
            .expect("loop has members");
        a.issues.push(Issue {
            level: Level::Problem,
            title: format!("{} depend on each other in a loop", names(mods, &looped)),
            detail: "The game switches every mod in a dependency loop off for the session and shows a pop-up. Usually one of them is outdated.".into(),
            fix: Some((format!("Keep {} only", mods[keep].name), Fix::Disable(looped.iter().copied().filter(|&i| i != keep).collect()))),
            automatic: false,
        });
    }

    // who (transitively) loads after whom
    let mut after: HashMap<usize, HashSet<usize>> = HashMap::new();
    for &i in &a.order {
        let mut seen = HashSet::new();
        let mut stack = deps_of(i);
        while let Some(d) = stack.pop() {
            if seen.insert(d) {
                stack.extend(deps_of(d));
            }
        }
        after.insert(i, seen);
    }

    // files replaced by more than one mod
    let mut by_target: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for &i in &a.order {
        for (target, _) in &mods[i].overrides {
            let list = by_target.entry(target.as_str()).or_default();
            if !list.contains(&i) {
                list.push(i);
            }
        }
    }
    let source = |i: usize, target: &str| mods[i].overrides.iter().find(|(t, _)| t == target).map(|(_, s)| s.clone());
    for (target, list) in by_target.into_iter().filter(|(_, l)| l.len() > 1) {
        let last = *list.last().expect("non-empty");
        let settled = list.iter().all(|&o| o == last || after[&last].contains(&o));
        let winner = if settled {
            last
        } else {
            // the player's pick, else the most recently updated mod; our own
            // speed patch gives way to mods that change behaviour
            pins.get(target)
                .and_then(|ns| list.iter().copied().find(|&i| &mods[i].namespace == ns))
                .unwrap_or_else(|| {
                    *list
                        .iter()
                        .max_by_key(|&&i| (mods[i].namespace != PATCH_NS, mods[i].modified))
                        .expect("non-empty")
                })
        };
        if !settled {
            if let Some(src) = source(winner, target) {
                a.wanted_overrides.insert(target.to_string(), src);
                for &i in &list {
                    a.wanted_dependencies.insert(mods[i].namespace.clone());
                }
            }
        }
        a.conflicts.push(Conflict { target: target.to_string(), mods: list, winner, settled });
    }

    let unsettled: Vec<&Conflict> = a.conflicts.iter().filter(|c| !c.settled).collect();
    let wanted = load_order_manifest(&a.wanted_overrides, &a.wanted_dependencies);
    let have = current_load_order;
    let up_to_date = match (have, a.wanted_overrides.is_empty()) {
        (None, true) => true,
        (Some(have), false) => same_load_order(have, &wanted),
        _ => false,
    };
    if !up_to_date {
        let (title, detail, label) = if unsettled.is_empty() {
            ("ReHearth Load Order is no longer needed".to_string(), "No mods fight over the same files any more.".to_string(), "Remove it")
        } else {
            let mut involved: Vec<usize> = unsettled.iter().flat_map(|c| c.mods.iter().copied()).collect();
            involved.sort();
            involved.dedup();
            (
                format!("{} {} replace the same {}", names(mods, &involved), if involved.len() == 2 { "both" } else { "all" },
                    if unsettled.len() == 1 { "file".to_string() } else { format!("{} files", unsettled.len()) }),
                "Nothing decides which copy the game uses, so it can change between launches. ReHearth pins one copy per file (you can pick which below).".to_string(),
                "Pin the winners",
            )
        };
        a.issues.push(Issue {
            level: if unsettled.is_empty() { Level::Note } else { Level::Warning },
            title,
            detail,
            fix: Some((label.into(), Fix::WriteLoadOrder)),
            automatic: true,
        });
    }

    a.issues.sort_by_key(|i| i.level);
    a
}

fn load_order_manifest(overrides: &BTreeMap<String, String>, deps: &BTreeSet<String>) -> Value {
    let overrides: Map<String, Value> = overrides.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
    json!({
        "info": {
            "name": "ReHearth Load Order",
            "namespace": LOAD_ORDER_NS,
            "version": 3,
            "description": "Generated by the ReHearth launcher. Loads after the mods below and pins which copy of each contested file the game uses. Regenerated whenever your mods change."
        },
        "dependencies": deps.iter().collect::<Vec<_>>(),
        "overrides": overrides,
    })
}

fn same_load_order(have: &Value, want: &Value) -> bool {
    have.get("overrides") == want.get("overrides") && have.get("dependencies") == want.get("dependencies")
}

/// The manifest of the generated mod as it is on disk, if it's there.
pub fn current(paths: &GamePaths) -> Option<Value> {
    let text = fs::read_to_string(paths.mods().join(LOAD_ORDER_NS).join("manifest.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// Writes (or removes, when there's nothing to pin) the generated mod.
pub fn write(paths: &GamePaths, analysis: &Analysis, settings: &mut UserSettings) -> Result<()> {
    let dir = paths.mods().join(LOAD_ORDER_NS);
    if analysis.wanted_overrides.is_empty() {
        if dir.join(mods::MANAGED_MARKER).exists() {
            fs::remove_dir_all(&dir)?;
        }
        return Ok(());
    }
    let manifest = load_order_manifest(&analysis.wanted_overrides, &analysis.wanted_dependencies);
    let text = serde_json::to_string_pretty(&manifest)?;
    mods::install_bundled(paths, LOAD_ORDER_NS, &[("manifest.json", text.as_str())], "generated")?;
    settings.set(&format!("mods.directory.{LOAD_ORDER_NS}.enabled"), Value::Bool(true));
    Ok(())
}

/// Applies fixes that only touch settings or the generated mod. Workshop
/// installs are handed back to the caller. Returns what changed, in words.
pub fn apply(
    fixes: &[(String, Fix)],
    mods: &[ModInfo],
    paths: &GamePaths,
    settings: &mut UserSettings,
    analysis: &Analysis,
) -> Result<(Vec<String>, Vec<(u64, String)>)> {
    let mut changes = Vec::new();
    let mut installs = Vec::new();
    for (_, fix) in fixes {
        match fix {
            Fix::Enable(list) => {
                for &i in list {
                    mods[i].set_enabled(settings, true);
                }
                changes.push(format!("switched on {}", names(mods, list)));
            }
            Fix::Disable(list) => {
                for &i in list {
                    mods[i].set_enabled(settings, false);
                }
                changes.push(format!("switched off {}", names(mods, list)));
            }
            Fix::InstallWorkshop(id, label) => installs.push((*id, label.clone())),
            Fix::WriteLoadOrder => {
                write(paths, analysis, settings)?;
                changes.push(if analysis.wanted_overrides.is_empty() {
                    "removed ReHearth Load Order".to_string()
                } else {
                    format!("pinned {} contested file{}", analysis.wanted_overrides.len(), if analysis.wanted_overrides.len() == 1 { "" } else { "s" })
                });
            }
        }
    }
    settings.save()?;
    Ok((changes, installs))
}

/// Applies the automatic fixes until nothing changes (switching a mod on can
/// reveal what *it* needs). Returns everything that changed.
pub fn auto_fix(
    paths: &GamePaths,
    pins: &BTreeMap<String, String>,
) -> Result<(Vec<String>, Vec<(u64, String)>)> {
    let mut all_changes = Vec::new();
    let mut all_installs = Vec::new();
    for _ in 0..4 {
        let mods = mods::scan(paths);
        let mut settings = UserSettings::load(paths)?;
        let analysis = analyze(&mods, &settings, pins, current(paths).as_ref());
        let fixes = analysis.automatic_fixes();
        if fixes.is_empty() {
            break;
        }
        let (changes, installs) = apply(&fixes, &mods, paths, &mut settings, &analysis)?;
        all_changes.extend(changes);
        all_installs.extend(installs);
    }
    Ok((all_changes, all_installs))
}

/// Short path for display: "stonehearth/ai/actions/foo.lua" -> "ai/actions/foo.lua (stonehearth)".
pub fn pretty_target(target: &str) -> String {
    match target.split_once('/') {
        Some((ns, rest)) => format!("{rest}  ({ns})"),
        None => target.to_string(),
    }
}

pub fn is_generated(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n == LOAD_ORDER_NS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn m(ns: &str, kind: ModKind, deps: &[&str], overrides: &[(&str, &str)], modified: u64) -> ModInfo {
        ModInfo {
            namespace: ns.into(),
            name: ns.into(),
            kind,
            path: PathBuf::from(format!("/x/{ns}")),
            default_enabled: true,
            debug: false,
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            managed: None,
            workshop_id: None,
            overrides: overrides.iter().map(|(t, s)| (t.to_string(), s.to_string())).collect(),
            api_version: Some(3),
            mod_version: None,
            manifest_error: None,
            modified,
            client_only: false,
        }
    }

    fn settings(json: &str) -> UserSettings {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("rehearth-lo-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("user_settings.json"), json).unwrap();
        let s = UserSettings::load(&GamePaths { game: dir.clone(), workshop: dir.join("w") }).unwrap();
        fs::remove_dir_all(dir).unwrap();
        s
    }

    #[test]
    fn orders_by_dependencies_and_flags_switched_off_requirements() {
        let mods = vec![
            m("stonehearth", ModKind::Base, &[], &[], 1),
            m("radiant", ModKind::Base, &[], &[], 1),
            m("northern_alliance", ModKind::Base, &[], &[], 1),
            m("zz_addon", ModKind::Workshop, &["stonehearth_ace"], &[], 1),
            m("stonehearth_ace", ModKind::Workshop, &["stonehearth", "northern_alliance", "metaclass_override"], &[], 1),
            m("aa_other", ModKind::Workshop, &[], &[], 1),
        ];
        let s = settings(r#"{"mods":{"base":{"northern_alliance":{"enabled":false}}}}"#);
        let a = analyze(&mods, &s, &BTreeMap::new(), None);
        let order: Vec<&str> = a.order.iter().map(|&i| mods[i].namespace.as_str()).collect();
        assert_eq!(order, ["radiant", "stonehearth", "aa_other", "stonehearth_ace", "zz_addon"]);
        let issue = &a.issues[0];
        assert_eq!(issue.fix.as_ref().unwrap().1, Fix::Enable(vec![2]));
        assert!(issue.automatic);
        // metaclass_override is only a hint, so it's never reported missing
        assert!(!a.issues.iter().any(|i| i.title.contains("metaclass")));
    }

    #[test]
    fn unsettled_conflicts_get_pinned_and_settled_ones_dont() {
        let t = "stonehearth/ai/foo.lua";
        let mods = vec![
            m("stonehearth", ModKind::Base, &[], &[], 1),
            m("stonehearth_ace", ModKind::Workshop, &["stonehearth"], &[(t, "stonehearth_ace/ai/foo.lua")], 5),
            // depends on ACE, so it wins over ACE without help
            m("ace_addon", ModKind::Workshop, &["stonehearth_ace"], &[(t, "ace_addon/foo.lua")], 1),
            // no relation to ace_addon: luck decides between them
            m("other", ModKind::Workshop, &[], &[(t, "other/foo.lua")], 9),
            m("rehearth_patch", ModKind::Directory, &["stonehearth_ace"], &[(t, "rehearth_patch/foo.lua")], 99),
        ];
        let s = settings("{}");
        let a = analyze(&mods, &s, &BTreeMap::new(), None);
        let c = &a.conflicts[0];
        assert!(!c.settled);
        // most recently updated wins, but our own patch steps aside
        assert_eq!(mods[c.winner].namespace, "other");
        assert_eq!(a.wanted_overrides[t], "other/foo.lua");
        assert!(a.wanted_dependencies.contains("ace_addon") && a.wanted_dependencies.contains("other"));
        assert!(a.issues.iter().any(|i| i.fix.as_ref().is_some_and(|f| f.1 == Fix::WriteLoadOrder)));

        // the player's pick beats the heuristic
        let mut pins = BTreeMap::new();
        pins.insert(t.to_string(), "ace_addon".to_string());
        let a = analyze(&mods, &s, &pins, None);
        assert_eq!(a.wanted_overrides[t], "ace_addon/foo.lua");

        // and once the generated mod matches, there's nothing left to do
        let wanted = load_order_manifest(&a.wanted_overrides, &a.wanted_dependencies);
        let a = analyze(&mods, &s, &pins, Some(&wanted));
        assert!(a.issues.is_empty(), "{:?}", a.issues);
    }

    #[test]
    fn duplicates_loops_and_broken_manifests() {
        let mut broken = m("broken", ModKind::Directory, &[], &[], 1);
        broken.manifest_error = Some("manifest.json is broken".into());
        let mods = vec![
            m("stonehearth", ModKind::Base, &[], &[], 1),
            m("dup", ModKind::Directory, &[], &[], 1),
            m("dup", ModKind::Workshop, &[], &[], 2),
            m("a", ModKind::Workshop, &["b"], &[], 1),
            m("b", ModKind::Workshop, &["a"], &[], 1),
            broken,
        ];
        let s = settings("{}");
        let a = analyze(&mods, &s, &BTreeMap::new(), None);
        let fix_of = |word: &str| a.issues.iter().find(|i| i.title.contains(word)).and_then(|i| i.fix.clone()).map(|f| f.1);
        assert_eq!(fix_of("installed 2 times"), Some(Fix::Disable(vec![1])));
        assert_eq!(fix_of("can't be read"), Some(Fix::Disable(vec![5])));
        assert!(fix_of("loop").is_some());
        assert!(!a.order.contains(&3) && !a.order.contains(&4));
    }
}
