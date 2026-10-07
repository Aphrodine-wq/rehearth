//! ReHearth for coding agents: the launcher's abilities as tools.
//!
//! Two ways in, sharing one set of tools:
//! - `rehearth mcp` speaks the Model Context Protocol over stdio, for Claude Code
//!   and any other MCP client (`claude mcp add rehearth -- rehearth mcp`).
//! - `rehearth api [tool] [json]` runs one tool and prints its JSON result, for
//!   anything that can run a command. With no tool it lists them.
//!
//! Every result is JSON. Tools that change things refuse while the game runs,
//! because Stonehearth rewrites its settings file when it exits.

use std::collections::HashSet;
use std::fs;
use std::io::{self, BufRead, Write};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::game::{self, GamePaths, UserSettings};
use crate::loadorder::{self, Level};
use crate::mods::{self, ModInfo};
use crate::workshop::{self, Item, Sort};
use crate::{Config, PATCH_FILES, PATCH_VERSION, bisect, doctor, needed, steam, update};

const INSTRUCTIONS: &str = "ReHearth manages Stonehearth (a Steam game, run through Proton) and its mods on this Linux machine. \
Start with `status`, then `health` to see mod problems. Mods are identified by namespace (e.g. \"stonehearth_ace\"); \
Workshop items by numeric id. Tools that change mods or settings fail while the game is running. Installs and removals \
go through the user's running Steam client and subscribe or unsubscribe their Steam account, so only do them when the \
user asked. Never launch the game unless the user asked you to.";

/// name, description, input schema
fn tool_list() -> Vec<(&'static str, &'static str, Value)> {
    let none = json!({"type": "object", "properties": {}});
    vec![
        ("status", "Where Stonehearth is, its version, whether it or Steam is running, how many mods are on, and how many problems the health check finds.", none.clone()),
        ("list_mods", "Every installed mod: namespace, name, source (Built-in, Workshop, Local, ReHearth), on/off, load position, Workshop id, version and dependencies.",
            json!({"type": "object", "properties": {"include_debug": {"type": "boolean", "description": "Also list debug/test mods. Default false."}}})),
        ("health", "The mod health check: problems and warnings with suggested fixes, files two mods both replace (and which copy wins), missing dependencies, and the load order.", none.clone()),
        ("fix_mods", "Applies the safe automatic fixes (the same ones ReHearth runs before Play): switches on mods other mods need, keeps the newest copy of duplicates, switches off unreadable mods, and pins contested files. Never deletes anything.", none.clone()),
        ("set_mod_enabled", "Switches one mod on or off in the game's user_settings.json.",
            json!({"type": "object", "properties": {"namespace": {"type": "string"}, "enabled": {"type": "boolean"}}, "required": ["namespace", "enabled"]})),
        ("find_needed_mods", "Looks up, on the Workshop, the mods that switched-on mods depend on but that aren't installed. Matches by title, so treat results as candidates. Installs nothing.", none.clone()),
        ("install_needed_mods", "Finds and installs missing dependencies through Steam, then checks each download really is the needed mod and removes it again if not. Subscribes the user's Steam account; can take minutes.",
            json!({"type": "object", "properties": {"namespaces": {"type": "array", "items": {"type": "string"}, "description": "Only these missing namespaces. Default: all of them."}}})),
        ("search_workshop", "Searches or browses the Stonehearth Workshop (30 items a page).",
            json!({"type": "object", "properties": {
                "query": {"type": "string", "description": "Search text. Empty browses by `sort`."},
                "sort": {"type": "string", "enum": ["trending", "popular", "updated", "newest"]},
                "page": {"type": "integer", "minimum": 1}}})),
        ("workshop_item", "Full details of Workshop items, including the description and whether each is installed.",
            json!({"type": "object", "properties": {"ids": {"type": "array", "items": {"type": "integer"}}}, "required": ["ids"]})),
        ("install_workshop_item", "Subscribes to a Workshop item through Steam and waits until it's downloaded. Returns the mod's namespace.",
            json!({"type": "object", "properties": {"id": {"type": "integer"}}, "required": ["id"]})),
        ("remove_workshop_item", "Unsubscribes from a Workshop item through Steam, which deletes its files.",
            json!({"type": "object", "properties": {"id": {"type": "integer"}}, "required": ["id"]})),
        ("last_session", "What happened in the last game session, from stonehearth.log: times, whether it closed cleanly, GPU, whether ReHearth Patch loaded, and errors grouped with the mod that likely caused each and a plain explanation.",
            json!({"type": "object", "properties": {"max_errors": {"type": "integer", "minimum": 0, "description": "Default 30."}}})),
        ("read_log", "The last lines of stonehearth.log.",
            json!({"type": "object", "properties": {"lines": {"type": "integer", "minimum": 1, "description": "Default 200."}}})),
        ("get_game_setting", "Reads a key from the game's user_settings.json, with dots for nesting (e.g. \"renderer.draw_distance\"). No key returns the whole file.",
            json!({"type": "object", "properties": {"key": {"type": "string"}}})),
        ("set_game_setting", "Writes a key in the game's user_settings.json (dots for nesting). ReHearth keeps a backup of the original file.",
            json!({"type": "object", "properties": {"key": {"type": "string"}, "value": {}}, "required": ["key", "value"]})),
        ("install_patch", "Installs or updates ReHearth Patch, the lag-fix mod bundled with ReHearth (needs ACE).", none.clone()),
        ("launch_game", "Starts Stonehearth through Steam. Only call this when the user asked to play or test. `safe` runs the base game only, for this session.",
            json!({"type": "object", "properties": {"mode": {"type": "string", "enum": ["normal", "safe"]}}})),
    ]
}

fn tools_json() -> Value {
    Value::Array(
        tool_list()
            .into_iter()
            .map(|(name, description, schema)| json!({"name": name, "description": description, "inputSchema": schema}))
            .collect(),
    )
}

// ---------------------------------------------------------------- helpers

fn paths(cfg: &Config) -> Result<GamePaths> {
    game::locate(cfg.game_dir.as_deref()).context("Stonehearth wasn't found. Set its folder in ReHearth's Settings.")
}

fn not_running() -> Result<()> {
    if game::is_running() {
        bail!("Stonehearth is running. Close it first: it rewrites its settings when it exits.");
    }
    Ok(())
}

fn steam_running() -> bool {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    [".steam/steam.pid", ".var/app/com.valvesoftware.Steam/.steam/steam.pid"].iter().any(|f| {
        fs::read_to_string(home.join(f))
            .ok()
            .and_then(|pid| pid.trim().parse::<u32>().ok())
            .is_some_and(|pid| std::path::Path::new(&format!("/proc/{pid}")).exists())
    })
}

fn installed_ids(paths: &GamePaths) -> HashSet<u64> {
    fs::read_dir(&paths.workshop)
        .map(|d| d.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect())
        .unwrap_or_default()
}

struct Snapshot {
    cfg: Config,
    paths: GamePaths,
    mods: Vec<ModInfo>,
    settings: UserSettings,
    analysis: loadorder::Analysis,
}

fn snapshot() -> Result<Snapshot> {
    let cfg = Config::load();
    let paths = paths(&cfg)?;
    let mods = mods::scan(&paths);
    let settings = UserSettings::load(&paths)?;
    let analysis = loadorder::analyze(&mods, &settings, &cfg.conflict_winners, loadorder::current(&paths).as_ref());
    Ok(Snapshot { cfg, paths, mods, settings, analysis })
}

fn level(l: Level) -> &'static str {
    match l {
        Level::Problem => "problem",
        Level::Warning => "warning",
        Level::Note => "note",
    }
}

fn item_json(i: &Item, installed: &HashSet<u64>) -> Value {
    json!({
        "id": i.id,
        "title": i.title,
        "summary": i.summary,
        "subscribers": i.subscriptions,
        "size_bytes": i.file_size,
        "updated": i.time_updated,
        "stars": i.stars,
        "tags": i.tags,
        "requires": i.requires,
        "installed": installed.contains(&i.id),
        "url": format!("https://steamcommunity.com/sharedfiles/filedetails/?id={}", i.id),
    })
}

/// Runs one Steam worker job to the end.
fn steam_job(cmd: &'static str, id: u64) -> Result<()> {
    let rx = steam::spawn(cmd, id)?;
    for event in rx {
        match event {
            steam::Event::Done => return Ok(()),
            steam::Event::Failed(e) => bail!(e),
            _ => {}
        }
    }
    bail!("the Steam worker stopped unexpectedly")
}

/// The namespace of the installed Workshop item `id`, from a fresh scan.
fn namespace_of(paths: &GamePaths, id: u64) -> Option<String> {
    let id = id.to_string();
    mods::scan(paths).into_iter().find(|m| m.workshop_id.as_deref() == Some(id.as_str())).map(|m| m.namespace)
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn arg_u64(args: &Value, key: &str) -> Result<u64> {
    args.get(key)
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .with_context(|| format!("`{key}` must be a number"))
}

// ---------------------------------------------------------------- tools

pub fn call(name: &str, args: &Value) -> Result<Value> {
    match name {
        "status" => {
            let cfg = Config::load();
            let found = game::locate(cfg.game_dir.as_deref());
            let mut out = json!({
                "rehearth_version": update::current(),
                "game_found": found.is_some(),
                "game_running": game::is_running(),
                "steam_running": steam_running(),
            });
            if let Some(paths) = &found {
                out["game_dir"] = json!(paths.game);
                out["workshop_dir"] = json!(paths.workshop);
                out["game_version"] = json!(game::version(paths));
                if let Ok(s) = snapshot() {
                    let on = s.analysis.order.iter().filter(|&&i| s.mods[i].kind != mods::ModKind::Base).count();
                    out["mods_installed"] = json!(s.mods.len());
                    out["mods_on"] = json!(on);
                    out["problems"] = json!(s.analysis.problems());
                    out["warnings"] = json!(s.analysis.issues.iter().filter(|i| i.level == Level::Warning).count());
                    out["missing_dependencies"] = json!(s.analysis.missing.len());
                    out["ace_installed"] = json!(s.mods.iter().any(|m| m.namespace == "stonehearth_ace"));
                    out["patch_installed"] = json!(s.mods.iter().any(|m| m.namespace == "rehearth_patch"));
                }
            }
            Ok(out)
        }
        "list_mods" => {
            let s = snapshot()?;
            let debug = args.get("include_debug").and_then(Value::as_bool).unwrap_or(false);
            let list: Vec<Value> = s
                .mods
                .iter()
                .enumerate()
                .filter(|(_, m)| debug || !m.debug)
                .map(|(i, m)| {
                    json!({
                        "namespace": m.namespace,
                        "name": m.name,
                        "source": m.kind.label(),
                        "enabled": m.is_enabled(&s.settings),
                        "load_position": s.analysis.order.iter().position(|&o| o == i).map(|p| p + 1),
                        "workshop_id": m.workshop_id,
                        "version": m.mod_version,
                        "dependencies": m.dependencies,
                        "debug": m.debug,
                        "manifest_error": m.manifest_error,
                        "path": m.path,
                    })
                })
                .collect();
            Ok(json!({ "mods": list }))
        }
        "health" => {
            let s = snapshot()?;
            let ns = |i: usize| s.mods[i].namespace.clone();
            Ok(json!({
                "issues": s.analysis.issues.iter().map(|i| json!({
                    "level": level(i.level),
                    "title": i.title,
                    "detail": i.detail,
                    "fix": i.fix.as_ref().map(|f| f.0.clone()),
                    "fixed_by_fix_mods": i.automatic,
                })).collect::<Vec<_>>(),
                "contested_files": s.analysis.conflicts.iter().map(|c| json!({
                    "file": c.target,
                    "mods": c.mods.iter().map(|&i| ns(i)).collect::<Vec<_>>(),
                    "winner": ns(c.winner),
                    "settled_by_load_order": c.settled,
                })).collect::<Vec<_>>(),
                "missing_dependencies": s.analysis.missing.iter().map(|(dep, who)| json!({
                    "namespace": dep,
                    "needed_by": who.iter().map(|&i| ns(i)).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "load_order": s.analysis.order.iter().map(|&i| ns(i)).collect::<Vec<_>>(),
            }))
        }
        "fix_mods" => {
            not_running()?;
            let cfg = Config::load();
            let paths = paths(&cfg)?;
            let (changes, _) = loadorder::auto_fix(&paths, &cfg.conflict_winners)?;
            Ok(json!({ "changes": changes }))
        }
        "set_mod_enabled" => {
            not_running()?;
            let namespace = arg_str(args, "namespace").context("`namespace` is required")?;
            let on = args.get("enabled").and_then(Value::as_bool).context("`enabled` must be true or false")?;
            let mut s = snapshot()?;
            let m = s.mods.iter().find(|m| m.namespace == namespace).with_context(|| format!("no mod with namespace {namespace}"))?;
            if m.required() && !on {
                bail!("{} is a required base mod", m.name);
            }
            m.set_enabled(&mut s.settings, on);
            s.settings.save()?;
            Ok(json!({ "namespace": namespace, "enabled": on }))
        }
        "find_needed_mods" => {
            let s = snapshot()?;
            let missing: Vec<String> = s.analysis.missing.keys().cloned().collect();
            let found = needed::find(&missing, &s.cfg.needed_known, &s.cfg.needed_wrong)?;
            let installed = installed_ids(&s.paths);
            Ok(json!({
                "needed": found.iter().map(|f| json!({
                    "namespace": f.namespace,
                    "needed_by": s.analysis.missing.get(&f.namespace).map(|v| v.iter().map(|&i| s.mods[i].namespace.clone()).collect::<Vec<_>>()),
                    "candidate": f.item.as_ref().map(|i| item_json(i, &installed)),
                })).collect::<Vec<_>>(),
            }))
        }
        "install_needed_mods" => install_needed(args),
        "search_workshop" => {
            let sort = match arg_str(args, "sort").unwrap_or("trending") {
                "popular" => Sort::Popular,
                "updated" => Sort::Updated,
                "newest" => Sort::Newest,
                _ => Sort::Trending,
            };
            let page = args.get("page").and_then(Value::as_u64).unwrap_or(1).max(1) as u32;
            let result = workshop::browse(sort, arg_str(args, "query").unwrap_or(""), page)?;
            let installed = paths(&Config::load()).map(|p| installed_ids(&p)).unwrap_or_default();
            Ok(json!({
                "page": result.page,
                "total_pages": result.total_pages,
                "total_items": result.total_count,
                "items": result.items.iter().map(|i| item_json(i, &installed)).collect::<Vec<_>>(),
            }))
        }
        "workshop_item" => {
            let ids: Vec<u64> = args
                .get("ids")
                .and_then(Value::as_array)
                .context("`ids` must be a list of numbers")?
                .iter()
                .filter_map(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
                .collect();
            let installed = paths(&Config::load()).map(|p| installed_ids(&p)).unwrap_or_default();
            let items = workshop::details(&ids)?;
            Ok(json!({
                "items": items.iter().map(|(i, long)| {
                    let mut v = item_json(i, &installed);
                    v["description"] = json!(long);
                    v
                }).collect::<Vec<_>>(),
            }))
        }
        "install_workshop_item" => {
            let id = arg_u64(args, "id")?;
            let paths = paths(&Config::load())?;
            steam_job("subscribe", id)?;
            Ok(json!({ "id": id, "installed": true, "namespace": namespace_of(&paths, id) }))
        }
        "remove_workshop_item" => {
            let id = arg_u64(args, "id")?;
            steam_job("unsubscribe", id)?;
            Ok(json!({ "id": id, "removed": true }))
        }
        "last_session" => {
            let paths = paths(&Config::load())?;
            let r = doctor::read(&paths.log());
            if !r.found {
                return Ok(json!({ "found": false }));
            }
            let max = args.get("max_errors").and_then(Value::as_u64).unwrap_or(30) as usize;
            Ok(json!({
                "found": true,
                "game_version": r.version,
                "started": r.started,
                "ended": r.ended,
                "closed_cleanly": r.clean_exit,
                "gpu": r.gpu,
                "patch_loaded": r.patch_loaded,
                "duplicate_item_searches": r.duplicate_think_outputs,
                "error_kinds": r.errors.len(),
                "errors_total": r.errors.iter().map(|e| e.count).sum::<usize>(),
                "errors": r.errors.iter().take(max).map(|e| json!({
                    "kind": e.kind.label(),
                    "message": e.message,
                    "count": e.count,
                    "first_seen": e.first_seen,
                    "side": e.side,
                    "likely_mods": e.mods,
                    "location": e.location,
                    "explanation": doctor::explain(e),
                    "traceback": e.traceback.iter().take(12).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "last_lines": if r.clean_exit { Vec::new() } else { r.tail.clone() },
            }))
        }
        "read_log" => {
            let paths = paths(&Config::load())?;
            let n = args.get("lines").and_then(Value::as_u64).unwrap_or(200) as usize;
            Ok(json!({ "path": paths.log(), "lines": doctor::tail(&paths.log(), n) }))
        }
        "get_game_setting" => {
            let paths = paths(&Config::load())?;
            let settings = UserSettings::load(&paths)?;
            Ok(match arg_str(args, "key") {
                Some(key) => json!({ "key": key, "value": settings.get(key) }),
                None => json!({ "settings": serde_json::from_str::<Value>(&fs::read_to_string(paths.user_settings())?)? }),
            })
        }
        "set_game_setting" => {
            not_running()?;
            let key = arg_str(args, "key").context("`key` is required")?;
            let value = args.get("value").cloned().context("`value` is required")?;
            let paths = paths(&Config::load())?;
            let mut settings = UserSettings::load(&paths)?;
            settings.set(key, value.clone());
            settings.save()?;
            Ok(json!({ "key": key, "value": value }))
        }
        "install_patch" => {
            let paths = paths(&Config::load())?;
            mods::install_bundled(&paths, "rehearth_patch", PATCH_FILES, PATCH_VERSION)?;
            Ok(json!({ "installed": "rehearth_patch", "version": PATCH_VERSION }))
        }
        "launch_game" => {
            if game::is_running() {
                bail!("Stonehearth is already running");
            }
            let s = snapshot()?;
            let safe = arg_str(args, "mode") == Some("safe");
            let mut fixed = Vec::new();
            let session_args = if safe {
                bisect::safe_mode_args(&s.mods, &s.settings)
            } else {
                if s.cfg.auto_fix {
                    fixed = loadorder::auto_fix(&s.paths, &s.cfg.conflict_winners)?.0;
                }
                Vec::new()
            };
            game::launch(&s.cfg.extra_args, &session_args)?;
            Ok(json!({ "launched": true, "mode": if safe { "safe" } else { "normal" }, "fixed_first": fixed }))
        }
        other => bail!("unknown tool `{other}`"),
    }
}

/// Finds the missing dependencies, installs each candidate (and the items it
/// requires), and checks every download is the namespace it was picked for.
fn install_needed(args: &Value) -> Result<Value> {
    let mut s = snapshot()?;
    let mut wanted: Vec<String> = s.analysis.missing.keys().cloned().collect();
    if let Some(only) = args.get("namespaces").and_then(Value::as_array) {
        let only: HashSet<&str> = only.iter().filter_map(Value::as_str).collect();
        wanted.retain(|ns| only.contains(ns.as_str()));
    }
    if wanted.is_empty() {
        return Ok(json!({ "results": [], "note": "No missing dependencies to install." }));
    }
    let found = needed::find(&wanted, &s.cfg.needed_known, &s.cfg.needed_wrong)?;
    let mut installed = installed_ids(&s.paths);
    let mut results = Vec::new();
    for f in found {
        let Some(item) = f.item else {
            results.push(json!({ "namespace": f.namespace, "result": "not_found" }));
            continue;
        };
        let mut extras = Vec::new();
        let todo: Vec<u64> = item.requires.iter().copied().filter(|d| !installed.contains(d)).collect();
        for dep in todo.iter() {
            if steam_job("subscribe", *dep).is_ok() {
                installed.insert(*dep);
                extras.push(*dep);
            }
        }
        let outcome = match steam_job("subscribe", item.id) {
            Err(e) => json!({ "result": "failed", "error": format!("{e:#}") }),
            Ok(()) => {
                installed.insert(item.id);
                match namespace_of(&s.paths, item.id) {
                    Some(ns) if ns == f.namespace => {
                        s.cfg.needed_known.insert(f.namespace.clone(), item.id);
                        json!({ "result": "installed" })
                    }
                    got => {
                        s.cfg.needed_wrong.entry(f.namespace.clone()).or_default().push(item.id);
                        let removed = steam_job("unsubscribe", item.id).is_ok();
                        json!({ "result": "wrong_mod_removed", "actual_namespace": got, "removed": removed })
                    }
                }
            }
        };
        let mut row = json!({ "namespace": f.namespace, "workshop_id": item.id, "title": item.title, "also_installed": extras });
        if let (Some(row), Some(o)) = (row.as_object_mut(), outcome.as_object()) {
            row.extend(o.clone());
        }
        results.push(row);
        let _ = s.cfg.save();
    }
    Ok(json!({ "results": results }))
}

// ---------------------------------------------------------------- front ends

/// `rehearth api [tool] [json]`
pub fn run_cli(args: &[String]) -> i32 {
    let Some(tool) = args.first() else {
        println!("{}", serde_json::to_string_pretty(&json!({ "instructions": INSTRUCTIONS, "tools": tools_json() })).unwrap_or_default());
        return 0;
    };
    let input = match args.get(1).map(|s| serde_json::from_str::<Value>(s)) {
        None => json!({}),
        Some(Ok(v)) => v,
        Some(Err(e)) => {
            println!("{}", json!({ "error": format!("arguments aren't valid JSON: {e}") }));
            return 2;
        }
    };
    match call(tool, &input) {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            0
        }
        Err(e) => {
            println!("{}", json!({ "error": format!("{e:#}") }));
            1
        }
    }
}

const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

fn reply(out: &mut impl Write, id: &Value, result: Result<Value, (i64, String)>) {
    let msg = match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    };
    let _ = writeln!(out, "{msg}");
    let _ = out.flush();
}

/// `rehearth mcp`: a Model Context Protocol server on stdin/stdout.
pub fn serve_mcp() -> i32 {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let names: HashSet<&str> = tool_list().iter().map(|t| t.0).collect();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                reply(&mut stdout, &Value::Null, Err((-32700, format!("parse error: {e}"))));
                continue;
            }
        };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
        let Some(id) = msg.get("id").cloned() else {
            continue; // a notification (e.g. notifications/initialized): nothing to answer
        };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
                let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(PROTOCOL_VERSIONS[0]);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "rehearth", "title": "ReHearth", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools_json() })),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
                if !names.contains(name) {
                    Err((-32602, format!("unknown tool `{name}`")))
                } else {
                    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                    Ok(match call(name, &args) {
                        Ok(v) => json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() }] }),
                        Err(e) => json!({ "content": [{ "type": "text", "text": format!("{e:#}") }], "isError": true }),
                    })
                }
            }
            "resources/list" => Ok(json!({ "resources": [] })),
            "prompts/list" => Ok(json!({ "prompts": [] })),
            other => Err((-32601, format!("method not found: {other}"))),
        };
        reply(&mut stdout, &id, result);
    }
    0
}
