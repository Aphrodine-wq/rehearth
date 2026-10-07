//! ReHearth: a launcher and mod manager for Stonehearth on Linux.

mod agent;
mod art;
mod bisect;
mod doctor;
mod game;
mod loadorder;
mod mods;
mod needed;
mod saves;
mod shop;
mod steam;
mod tabs;
mod theme;
mod update;
mod workshop;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Align2, Color32, FontId, RichText, Sense, pos2, vec2};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use bisect::Bisect;
use game::{GamePaths, UserSettings};
use loadorder::{Analysis, Fix};
use mods::{ModInfo, ModKind};
use shop::{Shop, ShopAction};
use theme::*;

const PATCH_VERSION: &str = "0.1.0";
const PATCH_FILES: &[(&str, &str)] = &[
    ("manifest.json", include_str!("../../mods/rehearth_patch/manifest.json")),
    ("rehearth_patch_server.lua", include_str!("../../mods/rehearth_patch/rehearth_patch_server.lua")),
    (
        "ai/actions/find_reachable_entity_type_anywhere.lua",
        include_str!("../../mods/rehearth_patch/ai/actions/find_reachable_entity_type_anywhere.lua"),
    ),
    ("README.md", include_str!("../../mods/rehearth_patch/README.md")),
    ("LICENSE.ace.md", include_str!("../../mods/rehearth_patch/LICENSE.ace.md")),
];

const ACE_GIT: &str = "https://github.com/StonehearthACE-team/stonehearth_ace.git";

const MENU_WIDTH: f32 = 280.0;

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize)]
struct Config {
    game_dir: Option<PathBuf>,
    #[serde(default)]
    extra_args: String,
    #[serde(default)]
    close_on_launch: bool,
    /// fix mod problems (switched-off requirements, duplicates, contested
    /// files) automatically when PLAY is pressed
    #[serde(default = "yes")]
    auto_fix: bool,
    /// the player's picks for contested files: target path -> namespace
    #[serde(default)]
    conflict_winners: BTreeMap<String, String>,
    /// a "find the problem mod" run in progress
    #[serde(default)]
    bisect: Option<Bisect>,
    /// look for a newer ReHearth on GitHub at startup
    #[serde(default = "yes")]
    check_updates: bool,
    /// Workshop items confirmed to provide a namespace
    #[serde(default)]
    needed_known: BTreeMap<String, u64>,
    /// Workshop items found not to be the namespace they were picked for
    #[serde(default)]
    needed_wrong: BTreeMap<String, Vec<u64>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            game_dir: None,
            extra_args: String::new(),
            close_on_launch: false,
            auto_fix: true,
            conflict_winners: BTreeMap::new(),
            bisect: None,
            check_updates: true,
            needed_known: BTreeMap::new(),
            needed_wrong: BTreeMap::new(),
        }
    }
}

impl Config {
    fn path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
        base.join("rehearth/config.json")
    }

    fn load() -> Self {
        fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn save(&self) -> anyhow::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Home,
    Mods,
    Workshop,
    Saves,
    Doctor,
    Settings,
}

impl Tab {
    const ALL: [Tab; 6] = [Tab::Home, Tab::Mods, Tab::Workshop, Tab::Saves, Tab::Doctor, Tab::Settings];

    fn label(self) -> &'static str {
        match self {
            Tab::Home => "Home",
            Tab::Mods => "Mods",
            Tab::Workshop => "Workshop",
            Tab::Saves => "Saves",
            Tab::Doctor => "Doctor",
            Tab::Settings => "Settings",
        }
    }
}

enum TaskResult {
    Message(String),
    Updates(Vec<String>),
    /// ReHearth replaced itself; restart from this path
    SelfUpdated(PathBuf),
    /// the Workshop lookup for mods that other mods need
    Needed(Vec<needed::Found>),
}

/// A row in the "needed mods" list: what was found, and whether to install it.
struct NeededRow {
    found: needed::Found,
    install: bool,
}

struct Task {
    label: String,
    rx: Receiver<Result<TaskResult, String>>,
}

/// How a launch should go.
#[derive(Clone, Copy, PartialEq)]
enum LaunchMode {
    Normal,
    /// base game only, for this session
    Safe,
    /// the next "find the problem mod" test
    Test,
}

enum Action {
    Launch(LaunchMode),
    ToggleMod(usize, bool),
    ApplyFixes(Vec<(String, Fix)>),
    PinWinner(String, String),
    RemoveMod(usize),
    OpenPath(PathBuf),
    InstallAceFromGit,
    InstallPatch,
    CheckUpdates,
    BackupSaves,
    RestoreBackup(PathBuf),
    Copy(String, &'static str),
    SetSetting(String, Value),
    RemoveSetting(String),
    RestoreSettingsBackup,
    SaveConfig,
    Refresh,
    Tab(Tab),
    ShowWorkshopItem(u64),
    StartBisect(bisect::Target),
    BisectAnswer(bool),
    StopBisect,
    SelfUpdate(String),
    RestartUpdated,
    AddToAppMenu,
    /// look up mods that other mods need (namespaces) on the Workshop
    FindNeeded(Vec<String>),
    InstallNeeded,
    CancelNeeded,
    SearchWorkshop(String),
}

struct App {
    cfg: Config,
    paths: Option<GamePaths>,
    version: Option<String>,
    settings: Option<UserSettings>,
    settings_error: Option<String>,
    mods: Vec<ModInfo>,
    analysis: Analysis,
    /// Workshop item ids that are on disk
    installed_ids: HashSet<u64>,
    saves: Vec<(saves::Save, String)>,
    backups: Vec<saves::Backup>,
    report: doctor::Report,
    tab: Tab,
    status: String,
    status_error: bool,
    task: Option<Task>,
    running: bool,
    last_poll: Instant,
    show_debug_mods: bool,
    confirm: Option<String>,
    updates: HashSet<String>,
    game_dir_edit: String,
    art: art::Art,
    shop: Shop,
    /// the log as the game writes it, while it runs
    live_log: Vec<String>,
    live_errors_only: bool,
    verbose_pick: String,
    /// the startup check for a newer ReHearth
    update_check: Option<Receiver<Option<String>>>,
    /// a newer ReHearth release, when there is one
    new_version: Option<String>,
    /// set once the new version is in place
    updated: Option<PathBuf>,
    /// the needed-mods lookup result, waiting for the player to confirm
    needed: Option<Vec<NeededRow>>,
    /// installs started for a needed namespace (Workshop id -> namespace), to check once they land
    needed_pending: HashMap<u64, String>,
}

fn date_cmd(ts: u64, fmt: &str) -> String {
    Command::new("date")
        .arg("-d")
        .arg(format!("@{ts}"))
        .arg(fmt)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn format_time(ts: u64) -> String {
    date_cmd(ts, "+%b %-d, %H:%M")
}

pub fn format_date(ts: u64) -> String {
    date_cmd(ts, "+%b %-d, %Y")
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        art::install_fonts(&cc.egui_ctx);
        let cfg = Config::load();
        let game = game::locate(cfg.game_dir.as_deref()).map(|p| p.game);
        let mut app = Self {
            game_dir_edit: cfg.game_dir.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            cfg,
            paths: None,
            version: None,
            settings: None,
            settings_error: None,
            mods: Vec::new(),
            analysis: Analysis::default(),
            installed_ids: HashSet::new(),
            saves: Vec::new(),
            backups: Vec::new(),
            report: doctor::Report::default(),
            tab: Tab::Home,
            status: String::new(),
            status_error: false,
            task: None,
            running: game::is_running(),
            last_poll: Instant::now(),
            show_debug_mods: false,
            confirm: None,
            updates: HashSet::new(),
            art: art::Art::load(game, &cc.egui_ctx),
            shop: Shop::new(&cc.egui_ctx),
            live_log: Vec::new(),
            live_errors_only: false,
            verbose_pick: String::new(),
            update_check: None,
            new_version: None,
            updated: None,
            needed: None,
            needed_pending: HashMap::new(),
        };
        if app.cfg.check_updates {
            let (tx, rx) = channel();
            thread::spawn(move || {
                let _ = tx.send(update::check().ok().flatten());
            });
            app.update_check = Some(rx);
        }
        app.refresh();
        // `rehearth --tab workshop` opens straight onto a screen
        let args: Vec<String> = std::env::args().collect();
        if let Some(name) = args.iter().position(|a| a == "--tab").and_then(|i| args.get(i + 1)) {
            if let Some(tab) = Tab::ALL.into_iter().find(|t| t.label().eq_ignore_ascii_case(name)) {
                app.tab = tab;
            }
        }
        app
    }

    fn refresh(&mut self) {
        self.paths = game::locate(self.cfg.game_dir.as_deref());
        let Some(paths) = &self.paths else {
            self.mods.clear();
            self.saves.clear();
            self.installed_ids.clear();
            self.settings = None;
            self.analysis = Analysis::default();
            return;
        };
        self.version = game::version(paths);
        match UserSettings::load(paths) {
            Ok(s) => {
                self.settings = Some(s);
                self.settings_error = None;
            }
            Err(e) => {
                self.settings = None;
                self.settings_error = Some(format!("{e:#}"));
            }
        }
        self.mods = mods::scan(paths);
        self.installed_ids = fs::read_dir(&paths.workshop)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| fs::read_dir(e.path()).is_ok_and(|mut d| d.next().is_some()))
                    .filter_map(|e| e.file_name().to_str()?.parse().ok())
                    .collect()
            })
            .unwrap_or_default();
        self.saves = saves::list(&paths.saves())
            .into_iter()
            .map(|s| {
                let when = format_time(s.modified);
                (s, when)
            })
            .collect();
        self.backups = saves::list_backups();
        self.report = doctor::read(&paths.log());
        self.reanalyze();
    }

    /// Re-runs the load-order check after a settings change (no disk rescan).
    fn reanalyze(&mut self) {
        let (Some(paths), Some(settings)) = (&self.paths, &self.settings) else {
            self.analysis = Analysis::default();
            return;
        };
        self.analysis = loadorder::analyze(&self.mods, settings, &self.cfg.conflict_winners, loadorder::current(paths).as_ref());
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_error = false;
    }

    fn fail(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_error = true;
    }

    fn save_config(&mut self) {
        if let Err(e) = self.cfg.save() {
            self.fail(format!("Couldn't save ReHearth's settings: {e:#}"));
        }
    }

    fn spawn<F>(&mut self, label: &str, work: F)
    where
        F: FnOnce() -> anyhow::Result<TaskResult> + Send + 'static,
    {
        if self.task.is_some() {
            self.fail("Wait for the current job to finish first.");
            return;
        }
        let (tx, rx) = channel();
        thread::spawn(move || {
            let _ = tx.send(work().map_err(|e| format!("{e:#}")));
        });
        self.task = Some(Task { label: label.to_string(), rx });
        self.say(format!("{label}…"));
    }

    fn poll(&mut self, ctx: &egui::Context) {
        self.art.poll(ctx);
        self.shop.poll(ctx);
        if std::mem::take(&mut self.shop.changed) {
            self.refresh();
        }
        for (job, ok) in std::mem::take(&mut self.shop.finished) {
            self.check_needed(job, ok);
        }
        if let Some((msg, err)) = self.shop.message.take() {
            if err { self.fail(msg) } else { self.say(msg) }
        }
        if let Some(rx) = &self.update_check {
            if let Ok(found) = rx.try_recv() {
                self.new_version = found;
                self.update_check = None;
            }
        }
        if let Some(task) = &self.task {
            if let Ok(result) = task.rx.try_recv() {
                self.task = None;
                match result {
                    Ok(TaskResult::Message(m)) => self.say(m),
                    Ok(TaskResult::Updates(names)) => {
                        if names.is_empty() {
                            self.say("Everything is up to date.");
                        } else {
                            self.say(format!("Updates available: {}", names.join(", ")));
                        }
                        self.updates = names.into_iter().collect();
                    }
                    Ok(TaskResult::Needed(found)) => {
                        let hits = found.iter().filter(|f| f.item.is_some()).count();
                        self.say(format!("Found {hits} of {} needed mods on the Workshop.", found.len()));
                        for f in &found {
                            if let Some(item) = &f.item {
                                self.shop.remember(item);
                            }
                        }
                        self.needed = Some(found.into_iter().map(|f| NeededRow { install: f.item.is_some(), found: f }).collect());
                        self.tab = Tab::Mods;
                    }
                    Ok(TaskResult::SelfUpdated(path)) => {
                        self.say("ReHearth is updated. Restart it to use the new version.");
                        self.updated = Some(path);
                    }
                    Err(e) => self.fail(e),
                }
                self.refresh();
            }
        }
        if self.last_poll.elapsed() > Duration::from_secs(2) {
            self.last_poll = Instant::now();
            let was_running = self.running;
            self.running = game::is_running();
            if self.running {
                if let Some(p) = &self.paths {
                    self.live_log = doctor::tail(&p.log(), 400);
                }
            }
            if was_running && !self.running {
                self.game_closed();
            }
        }
        let wait = if self.task.is_some() || self.running { 250 } else { 1000 };
        ctx.request_repaint_after(Duration::from_millis(wait));
    }

    /// A Workshop job ended. If it was a download for a needed mod, check the
    /// mod it brought is really that namespace; take it back out if not.
    fn check_needed(&mut self, job: shop::Job, ok: bool) {
        if job.kind != shop::JobKind::Install {
            return;
        }
        let Some(namespace) = self.needed_pending.remove(&job.id) else { return };
        if !ok {
            return;
        }
        let id = job.id.to_string();
        let got = self.mods.iter().find(|m| m.workshop_id.as_deref() == Some(id.as_str())).map(|m| m.namespace.clone());
        match got {
            Some(ns) if ns == namespace => {
                self.cfg.needed_known.insert(namespace, job.id);
                self.save_config();
            }
            Some(ns) => {
                self.cfg.needed_wrong.entry(namespace.clone()).or_default().push(job.id);
                self.save_config();
                self.shop.remove(job.id, job.title.clone());
                self.fail(format!(
                    "{} turned out to be \"{ns}\", not the \"{namespace}\" mod, so ReHearth is removing it again. Search the Workshop for the right one.",
                    job.title
                ));
            }
            // not readable yet (Steam may still be unpacking); leave it be
            None => {}
        }
    }

    /// The game just closed: read its log, and judge a problem-mod test if one ran.
    fn game_closed(&mut self) {
        self.refresh();
        if let Some(b) = &mut self.cfg.bisect {
            if b.launched {
                b.seen = Some(b.target.seen_in(&self.report));
                self.tab = Tab::Doctor;
                self.say("Test session over. Doctor has the result; confirm it to continue.");
                self.save_config();
                return;
            }
        }
        if self.report.found && !self.report.clean_exit {
            self.tab = Tab::Doctor;
            self.fail("The game didn't close normally. Doctor shows what it was doing last.");
        } else if !self.report.errors.is_empty() {
            self.say(format!("Game closed with {} kinds of error. See Doctor.", self.report.errors.len()));
        } else {
            self.say("Game closed cleanly.");
        }
    }

    fn find_mod(&self, namespace: &str) -> Option<&ModInfo> {
        self.mods.iter().find(|m| m.namespace == namespace)
    }

    fn mod_index(&self, namespace: &str) -> Option<usize> {
        // prefer the copy that's switched on
        let settings = self.settings.as_ref()?;
        self.mods
            .iter()
            .position(|m| m.namespace == namespace && m.is_enabled(settings))
            .or_else(|| self.mods.iter().position(|m| m.namespace == namespace))
    }

    fn display_name(&self, namespace: &str) -> String {
        self.find_mod(namespace).map(|m| m.name.clone()).unwrap_or_else(|| namespace.to_string())
    }

    fn launch(&mut self, mode: LaunchMode, ctx: &egui::Context) {
        let Some(paths) = &self.paths else { return };
        let mut session_args = Vec::new();
        let mut fixed = None;
        match mode {
            LaunchMode::Normal => {
                if self.cfg.auto_fix {
                    match loadorder::auto_fix(paths, &self.cfg.conflict_winners) {
                        Ok((changes, _)) if !changes.is_empty() => fixed = Some(changes.join("; ")),
                        Ok(_) => {}
                        Err(e) => {
                            self.fail(format!("Couldn't fix mods before launch: {e:#}"));
                            return;
                        }
                    }
                    if fixed.is_some() {
                        self.refresh();
                    }
                }
            }
            LaunchMode::Safe => {
                if let Some(s) = &self.settings {
                    session_args = bisect::safe_mode_args(&self.mods, s);
                }
            }
            LaunchMode::Test => {
                let Some(b) = &mut self.cfg.bisect else { return };
                if let Some(s) = &self.settings {
                    session_args = b.launch_args(&self.mods, s);
                }
                b.launched = true;
                self.save_config();
            }
        }
        match game::launch(&self.cfg.extra_args, &session_args) {
            Ok(()) => {
                self.running = true;
                self.live_log.clear();
                self.last_poll = Instant::now() + Duration::from_secs(10); // give Steam time
                self.say(match (mode, fixed) {
                    (LaunchMode::Normal, Some(f)) => format!("Fixed before launch: {f}. Starting Stonehearth…"),
                    (LaunchMode::Normal, None) => "Starting Stonehearth through Steam…".to_string(),
                    (LaunchMode::Safe, _) => "Starting Stonehearth in safe mode (base game only, this session)…".to_string(),
                    (LaunchMode::Test, _) => "Test session started. Play until the problem would show up, then quit.".to_string(),
                });
                if self.cfg.close_on_launch && mode == LaunchMode::Normal {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Err(e) => self.fail(format!("{e:#}")),
        }
    }

    fn apply(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::Launch(mode) => self.launch(mode, ctx),
            Action::ToggleMod(i, on) => {
                let Some(m) = self.mods.get(i).cloned() else { return };
                let Some(settings) = &mut self.settings else { return };
                m.set_enabled(settings, on);
                match settings.save() {
                    Ok(()) => self.say(format!("{} {}", m.name, if on { "switched on" } else { "switched off" })),
                    Err(e) => self.fail(format!("{e:#}")),
                }
                self.reanalyze();
            }
            Action::ApplyFixes(fixes) => {
                let wanted: Vec<String> = fixes
                    .iter()
                    .filter_map(|(_, f)| match f {
                        Fix::FindOnWorkshop(list) => Some(list.clone()),
                        _ => None,
                    })
                    .flatten()
                    .collect();
                if !wanted.is_empty() {
                    self.apply(Action::FindNeeded(wanted), ctx);
                }
                let fixes: Vec<_> = fixes.into_iter().filter(|(_, f)| !matches!(f, Fix::FindOnWorkshop(_))).collect();
                if fixes.is_empty() {
                    return;
                }
                let (Some(paths), Some(settings)) = (&self.paths, &mut self.settings) else { return };
                match loadorder::apply(&fixes, &self.mods, paths, settings, &self.analysis) {
                    Ok((changes, installs)) => {
                        for (id, _) in &installs {
                            self.shop.install(*id, &self.installed_ids);
                        }
                        let mut parts = changes;
                        parts.extend(installs.iter().map(|(_, label)| format!("downloading {label}")));
                        self.say(if parts.is_empty() { "Nothing to change.".to_string() } else { format!("Done: {}.", parts.join("; ")) });
                    }
                    Err(e) => self.fail(format!("{e:#}")),
                }
                self.refresh();
            }
            Action::PinWinner(target, ns) => {
                self.cfg.conflict_winners.insert(target, ns.clone());
                self.save_config();
                self.reanalyze();
                // keep the generated mod in step right away
                if let Some(fix) = self.analysis.issues.iter().find_map(|i| i.fix.clone().filter(|f| f.1 == Fix::WriteLoadOrder)) {
                    self.apply(Action::ApplyFixes(vec![fix]), ctx);
                }
                self.say(format!("{} now wins that file.", self.display_name(&ns)));
            }
            Action::RemoveMod(i) => {
                let Some(m) = self.mods.get(i).cloned() else { return };
                if m.kind == ModKind::Workshop {
                    match m.workshop_id.as_deref().and_then(|id| id.parse().ok()) {
                        Some(id) => {
                            self.shop.remove(id, m.name.clone());
                            self.say(format!("Removing {} through Steam…", m.name));
                        }
                        None => self.fail(format!("Couldn't tell which Workshop item {} is", m.name)),
                    }
                    return;
                }
                match mods::remove(&m) {
                    Ok(()) => self.say(format!("Removed {}", m.name)),
                    Err(e) => self.fail(format!("{e:#}")),
                }
                self.refresh();
            }
            Action::OpenPath(p) => game::open_path(&p),
            Action::InstallAceFromGit => {
                let Some(paths) = self.paths.as_ref().map(|p| GamePaths { game: p.game.clone(), workshop: p.workshop.clone() }) else {
                    return;
                };
                self.spawn("Downloading ACE from GitHub (about 1.3 GB)", move || {
                    let ns = mods::install_git(&paths, ACE_GIT, "stable-updating")?;
                    Ok(TaskResult::Message(format!("Installed ACE ({ns})")))
                });
            }
            Action::InstallPatch => {
                let Some(paths) = &self.paths else { return };
                match mods::install_bundled(paths, "rehearth_patch", PATCH_FILES, PATCH_VERSION) {
                    Ok(()) => self.say(format!("Installed ReHearth Patch {PATCH_VERSION}")),
                    Err(e) => self.fail(format!("{e:#}")),
                }
                self.refresh();
            }
            Action::CheckUpdates => {
                let targets: Vec<(String, String, String, String)> = self
                    .mods
                    .iter()
                    .filter_map(|m| {
                        let g = m.managed.as_ref()?;
                        (g.source.starts_with("http")).then(|| (m.namespace.clone(), g.source.clone(), g.branch.clone(), g.commit.clone()))
                    })
                    .collect();
                self.spawn("Checking for updates", move || {
                    let stale = targets
                        .into_iter()
                        .filter(|(_, url, branch, commit)| {
                            mods::remote_commit(url, branch).is_some_and(|remote| &remote != commit)
                        })
                        .map(|(ns, ..)| ns)
                        .collect();
                    Ok(TaskResult::Updates(stale))
                });
            }
            Action::BackupSaves => {
                let Some(dir) = self.paths.as_ref().map(|p| p.saves()) else { return };
                self.spawn("Backing up saves", move || {
                    let out = saves::back_up(&dir)?;
                    Ok(TaskResult::Message(format!("Saves backed up to {}", out.display())))
                });
            }
            Action::RestoreBackup(file) => {
                let Some(dir) = self.paths.as_ref().map(|p| p.saves()) else { return };
                self.spawn("Restoring saves", move || {
                    saves::restore(&dir, &file)?;
                    Ok(TaskResult::Message("Saves restored (your previous saves were backed up first).".into()))
                });
            }
            Action::Copy(text, what) => {
                ctx.copy_text(text);
                self.say(format!("Copied {what} to the clipboard."));
            }
            Action::SetSetting(key, value) => {
                let Some(settings) = &mut self.settings else { return };
                settings.set(&key, value);
                if let Err(e) = settings.save() {
                    self.fail(format!("{e:#}"));
                }
            }
            Action::RemoveSetting(key) => {
                let Some(settings) = &mut self.settings else { return };
                settings.remove(&key);
                if let Err(e) = settings.save() {
                    self.fail(format!("{e:#}"));
                }
            }
            Action::RestoreSettingsBackup => {
                let Some(paths) = &self.paths else { return };
                let original = paths.user_settings();
                let backup = original.with_extension("json.rehearth-bak");
                match fs::copy(&backup, &original) {
                    Ok(_) => self.say("Restored your original user_settings.json."),
                    Err(e) => self.fail(format!("No backup to restore ({e})")),
                }
                self.refresh();
            }
            Action::SaveConfig => {
                let dir = self.game_dir_edit.trim();
                self.cfg.game_dir = (!dir.is_empty()).then(|| PathBuf::from(dir));
                self.save_config();
                self.say("Settings saved.");
                self.refresh();
            }
            Action::Refresh => {
                self.refresh();
                self.say("Refreshed.");
            }
            Action::Tab(t) => {
                self.tab = t;
                self.confirm = None;
            }
            Action::ShowWorkshopItem(id) => {
                self.tab = Tab::Workshop;
                self.shop.open = Some(id);
            }
            Action::StartBisect(target) => {
                let b = Bisect::start(target, &self.mods, &self.analysis);
                if b.found.is_some() {
                    self.fail("You need at least two mods switched on to narrow anything down.");
                    return;
                }
                self.say(format!("Finding the problem mod: about {} test sessions.", b.rounds_left()));
                self.cfg.bisect = Some(b);
                self.save_config();
            }
            Action::BisectAnswer(seen) => {
                if let Some(b) = &mut self.cfg.bisect {
                    b.answer(seen);
                }
                self.save_config();
            }
            Action::StopBisect => {
                self.cfg.bisect = None;
                self.save_config();
                self.say("Stopped. Your mod settings were never changed.");
            }
            Action::SelfUpdate(version) => {
                self.spawn(&format!("Updating ReHearth to {version}"), move || {
                    Ok(TaskResult::SelfUpdated(update::install(&version)?))
                });
            }
            Action::RestartUpdated => {
                if let Some(path) = &self.updated {
                    update::restart(path);
                    std::process::exit(0);
                }
            }
            Action::FindNeeded(mut namespaces) => {
                namespaces.sort();
                namespaces.dedup();
                let (known, wrong) = (self.cfg.needed_known.clone(), self.cfg.needed_wrong.clone());
                let n = namespaces.len();
                self.spawn(&format!("Looking up {n} needed mod{} on the Workshop", if n == 1 { "" } else { "s" }), move || {
                    Ok(TaskResult::Needed(needed::find(&namespaces, &known, &wrong)?))
                });
            }
            Action::InstallNeeded => {
                let Some(rows) = self.needed.take() else { return };
                let mut titles = Vec::new();
                for row in rows.into_iter().filter(|r| r.install) {
                    let Some(item) = row.found.item else { continue };
                    self.needed_pending.insert(item.id, row.found.namespace);
                    self.shop.install(item.id, &self.installed_ids);
                    titles.push(item.title);
                }
                if titles.is_empty() {
                    self.say("Nothing picked to install.");
                } else {
                    self.say(format!("Downloading {} through Steam.", titles.join(", ")));
                }
            }
            Action::CancelNeeded => self.needed = None,
            Action::SearchWorkshop(text) => {
                self.tab = Tab::Workshop;
                self.shop.search_for(ctx, &text);
            }
            Action::AddToAppMenu => match update::add_to_app_menu() {
                Ok(_) => self.say("ReHearth is in your app menu now."),
                Err(e) => self.fail(format!("Couldn't add ReHearth to the app menu: {e:#}")),
            },
        }
    }
}

/// The PLAY button at the foot of the menu: always one click away.
fn play_button(ui: &mut egui::Ui, running: bool, enabled: bool) -> bool {
    let size = vec2(ui.available_width(), 68.0);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let painter = ui.painter();
    let hovered = enabled && response.hovered();
    let (top, bottom) = if !enabled {
        (Color32::from_rgb(0x5a, 0x4c, 0x3c), Color32::from_rgb(0x46, 0x3a, 0x2e))
    } else if hovered {
        (Color32::from_rgb(0xff, 0xc6, 0x58), Color32::from_rgb(0xf0, 0x94, 0x26))
    } else {
        (Color32::from_rgb(0xf8, 0xb8, 0x48), Color32::from_rgb(0xe0, 0x84, 0x1e))
    };
    if hovered {
        painter.rect_filled(rect.expand(4.0), 18, AMBER.gamma_multiply(0.22));
    }
    // rounded body, then a vertical gradient inset so the corners stay round
    painter.rect_filled(rect, 14, bottom);
    painter.rect_filled(egui::Rect::from_min_max(rect.min, pos2(rect.right(), rect.center().y)), 14, top);
    let mut mesh = egui::Mesh::default();
    let band = egui::Rect::from_min_max(pos2(rect.left(), rect.top() + 12.0), pos2(rect.right(), rect.bottom() - 12.0));
    mesh.colored_vertex(band.left_top(), top);
    mesh.colored_vertex(band.right_top(), top);
    mesh.colored_vertex(band.right_bottom(), bottom);
    mesh.colored_vertex(band.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
    let label = if running { "Running" } else { "Play" };
    let ink = if enabled { Color32::from_rgb(0x3a, 0x1d, 0x04) } else { DIM };
    painter.text(rect.center(), Align2::CENTER_CENTER, label, FontId::new(28.0, art::bold_family()), ink);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

/// One entry in the main menu.
fn menu_item(ui: &mut egui::Ui, text: &str, selected: bool, badge: Option<(String, Color32)>) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::click());
    let hovered = response.hovered();
    let painter = ui.painter();
    if selected || hovered {
        painter.rect_filled(rect, 10, if selected { Color32::from_white_alpha(24) } else { Color32::from_white_alpha(10) });
    }
    if selected {
        painter.rect_filled(egui::Rect::from_center_size(pos2(rect.left() + 3.0, rect.center().y), vec2(4.0, 20.0)), 2, AMBER);
    }
    let color = if selected { Color32::WHITE } else if hovered { TEXT } else { Color32::from_rgb(0xd8, 0xcc, 0xb8) };
    let font = FontId::new(18.5, art::medium_family());
    let end = painter.text(pos2(rect.left() + 18.0, rect.center().y), Align2::LEFT_CENTER, text, font, color);
    if let Some((badge, fill)) = badge {
        let font = FontId::new(12.0, art::bold_family());
        let galley = painter.layout_no_wrap(badge, font, BG);
        let r = egui::Rect::from_min_size(pos2(end.right() + 8.0, rect.center().y - 9.0), galley.size() + vec2(12.0, 4.0));
        painter.rect_filled(r, 10, fill);
        painter.galley(r.min + vec2(6.0, 2.0), galley, BG);
    }
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

impl App {
    fn menu(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.add_space(26.0);
        match &self.art.logo {
            Some(logo) => {
                let w = ui.available_width() - 20.0;
                let [lw, lh] = logo.size();
                ui.add(egui::Image::new(logo).fit_to_exact_size(vec2(w, w * lh as f32 / lw as f32)));
            }
            None => {
                ui.label(RichText::new("Stonehearth").font(FontId::new(30.0, art::bold_family())).color(AMBER));
            }
        }
        ui.add_space(2.0);
        ui.label(RichText::new("ReHearth · launcher & mod manager").color(DIM).size(13.5));
        ui.add_space(24.0);

        let problems = self.analysis.problems();
        for tab in Tab::ALL {
            let badge = match tab {
                Tab::Mods if problems > 0 => Some((problems.to_string(), BAD)),
                Tab::Doctor if self.cfg.bisect.is_some() => Some(("test".to_string(), AMBER)),
                Tab::Doctor if !self.report.errors.is_empty() => Some((self.report.errors.len().to_string(), AMBER)),
                Tab::Workshop => self.shop.downloading().map(|(_, p, queued)| {
                    let text = match p {
                        Some((d, t)) if t > 0 => format!("{:.0}%", d as f32 * 100.0 / t as f32),
                        _ => format!("{}", queued + 1),
                    };
                    (text, AMBER)
                }),
                _ => None,
            };
            if menu_item(ui, tab.label(), self.tab == tab, badge) {
                actions.push(Action::Tab(tab));
                if tab == Tab::Workshop {
                    self.shop.open = None;
                }
            }
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("ReHearth {}", update::current())).color(DIM).small());
                if let Some(v) = &self.version {
                    ui.label(RichText::new(format!("· Stonehearth {}", v.trim_end_matches(" (x64)"))).color(DIM).small());
                }
            });
            if self.updated.is_some() {
                if ui.button(RichText::new("Restart to finish updating").color(AMBER)).clicked() {
                    actions.push(Action::RestartUpdated);
                }
            } else if let Some(v) = &self.new_version {
                let install = update::Install::detect();
                if install.can_replace() {
                    let busy = self.task.is_some();
                    if ui.add_enabled(!busy, egui::Button::new(RichText::new(format!("Update to ReHearth {v}")).color(AMBER))).clicked() {
                        actions.push(Action::SelfUpdate(v.clone()));
                    }
                } else {
                    ui.label(RichText::new(format!("ReHearth {v} is out ({})", install.manual_hint())).color(AMBER).small());
                }
            }
            // status: download, background job, or the last message
            if let Some((title, progress, queued)) = self.shop.downloading() {
                let f = progress.filter(|p| p.1 > 0).map(|(d, t)| d as f32 / t as f32);
                let mut bar = egui::ProgressBar::new(f.unwrap_or(0.0)).fill(AMBER).desired_height(6.0);
                if f.is_none() {
                    bar = bar.animate(true);
                }
                ui.add(bar);
                let more = if queued > 0 { format!(" (+{queued} queued)") } else { String::new() };
                ui.label(RichText::new(format!("Steam: {title}{more}")).color(DIM).small());
            } else if let Some(task) = &self.task {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(format!("{}…", task.label)).color(DIM).small());
                });
            } else if !self.status.is_empty() {
                let mut job = egui::text::LayoutJob::simple(
                    self.status.clone(),
                    FontId::proportional(13.0),
                    if self.status_error { BAD } else { DIM },
                    ui.available_width(),
                );
                job.wrap.max_rows = 4;
                ui.label(job);
            }
            ui.add_space(8.0);
            let ready = self.paths.is_some() && !self.running && self.task.is_none();
            if let Some(hint) = self.launch_hint() {
                ui.label(RichText::new(hint).color(DIM).small());
            }
            if play_button(ui, self.running, ready) {
                actions.push(Action::Launch(LaunchMode::Normal));
            }
        });
    }

    /// One line above PLAY about what pressing it will do.
    fn launch_hint(&self) -> Option<String> {
        if self.paths.is_none() {
            return Some("Stonehearth wasn't found. Set its folder in Settings.".into());
        }
        if self.running {
            return None;
        }
        let fixes = self.analysis.automatic_fixes().len();
        let problems = self.analysis.problems();
        if fixes > 0 && self.cfg.auto_fix {
            Some(format!("Fixes {fixes} mod issue{} first", if fixes == 1 { "" } else { "s" }))
        } else if problems > 0 {
            Some(format!("{problems} mod problem{} (see Mods)", if problems == 1 { "" } else { "s" }))
        } else {
            None
        }
    }

    fn workshop_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        if self.shop.open.is_none() {
            heading(ui, "Workshop");
        }
        let has_patch = self.find_mod("rehearth_patch").is_some();
        let has_ace = self.find_mod("stonehearth_ace").is_some();
        let mut out = Vec::new();
        self.shop.ui(ui, &self.installed_ids, has_patch, has_ace, self.running, &mut out);
        for a in out {
            actions.push(match a {
                ShopAction::InstallPatch => Action::InstallPatch,
                ShopAction::InstallAceFromGit => Action::InstallAceFromGit,
            });
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        let mut actions = Vec::new();

        let screen = ui.max_rect();
        self.art.paint_background(&ctx, ui.painter(), screen);

        egui::Panel::left("menu")
            .exact_size(MENU_WIDTH)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.inner_margin(egui::Margin { left: 22, right: 16, top: 0, bottom: 0 }))
            .show(ui, |ui| self.menu(ui, &mut actions));

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(egui::Margin { left: 6, right: 22, top: 22, bottom: 22 }))
            .show(ui, |ui| match self.tab {
                Tab::Home => self.home_tab(ui, &mut actions),
                tab => panel(ui, |ui| match tab {
                    Tab::Mods => self.mods_tab(ui, &mut actions),
                    Tab::Workshop => self.workshop_tab(ui, &mut actions),
                    Tab::Saves => self.saves_tab(ui, &mut actions),
                    Tab::Doctor => self.doctor_tab(ui, &mut actions),
                    Tab::Settings => self.settings_tab(ui, &mut actions),
                    Tab::Home => {}
                }),
            });

        for action in actions {
            self.apply(action, &ctx);
        }
    }
}

/// `rehearth --report`: what the launcher sees, as text, without opening a window.
fn print_report() {
    let cfg = Config::load();
    let Some(paths) = game::locate(cfg.game_dir.as_deref()) else {
        println!("Stonehearth not found");
        return;
    };
    println!("game:     {}", paths.game.display());
    println!("workshop: {}", paths.workshop.display());
    println!("version:  {}", game::version(&paths).unwrap_or_else(|| "?".into()));
    println!("running:  {}", game::is_running());
    let settings = UserSettings::load(&paths);
    println!("\nmods:");
    for m in mods::scan(&paths) {
        let state = match &settings {
            Ok(s) if m.is_enabled(s) => "on ",
            Ok(_) => "off",
            Err(_) => "?  ",
        };
        println!(
            "  [{state}] {:<28} {:<12} {}{}",
            m.namespace,
            m.kind.label(),
            m.name,
            if m.debug { " (debug)" } else { "" }
        );
    }
    if let Ok(s) = &settings {
        let mods = mods::scan(&paths);
        let a = loadorder::analyze(&mods, s, &cfg.conflict_winners, loadorder::current(&paths).as_ref());
        let order: Vec<&str> = a.order.iter().map(|&i| mods[i].namespace.as_str()).collect();
        println!("\nload order: {}", order.join(" → "));
        for i in &a.issues {
            let auto = if i.automatic { " [auto-fix]" } else { "" };
            println!("  {:?}: {}{auto}\n      {}", i.level, i.title, i.detail);
        }
        for c in &a.conflicts {
            let who: Vec<&str> = c.mods.iter().map(|&i| mods[i].namespace.as_str()).collect();
            println!("  file {} ← {} (uses {}{})", c.target, who.join(", "), mods[c.winner].namespace, if c.settled { "" } else { ", pinned" });
        }
    }
    let saves = saves::list(&paths.saves());
    println!("\nsaves: {}  backups: {}", saves.len(), saves::list_backups().len());
    println!("\n{}", doctor::to_text(&doctor::read(&paths.log())));
}

fn window_icon() -> egui::IconData {
    let rgba = image::load_from_memory(update::ICON_PNG).map(|i| i.to_rgba8()).unwrap_or_default();
    egui::IconData { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--version") {
        println!("rehearth {}", update::current());
        return Ok(());
    }
    // for coding agents: an MCP server, or one tool call as JSON
    if matches!(args.first().map(String::as_str), Some("mcp" | "--mcp")) {
        std::process::exit(agent::serve_mcp());
    }
    if matches!(args.first().map(String::as_str), Some("api" | "--api")) {
        std::process::exit(agent::run_cli(&args[1..]));
    }
    if args.first().map(String::as_str) == Some("--update") {
        std::process::exit(update::run_cli());
    }
    if args.first().map(String::as_str) == Some("--browse") {
        // `rehearth --browse [search]`: the Workshop as the launcher sees it
        match workshop::browse(workshop::Sort::Trending, &args[1..].join(" "), 1) {
            Ok(page) => {
                println!("page {}/{} of {} mods", page.page, page.total_pages, page.total_count);
                for i in page.items {
                    println!("  {:>11} {:>7} subs {:>8}  {}{}", i.id, workshop::human_count(i.subscriptions),
                        workshop::human_size(i.file_size), i.title,
                        if i.requires.is_empty() { String::new() } else { format!("  (needs {:?})", i.requires) });
                }
            }
            Err(e) => eprintln!("{e:#}"),
        }
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("--steam-worker") {
        std::process::exit(steam::worker(&args[1..]));
    }
    if args.first().map(String::as_str) == Some("--fix") {
        // `rehearth --fix`: apply the automatic mod fixes without opening a window
        let cfg = Config::load();
        let Some(paths) = game::locate(cfg.game_dir.as_deref()) else {
            eprintln!("Stonehearth not found");
            std::process::exit(1);
        };
        match loadorder::auto_fix(&paths, &cfg.conflict_winners) {
            Ok((changes, _)) if changes.is_empty() => println!("Nothing to fix."),
            Ok((changes, _)) => changes.iter().for_each(|c| println!("{c}")),
            Err(e) => {
                eprintln!("{e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("--find-needed") {
        // `rehearth --find-needed`: what "Install all needed mods" would pick
        let cfg = Config::load();
        let Some(paths) = game::locate(cfg.game_dir.as_deref()) else {
            eprintln!("Stonehearth not found");
            std::process::exit(1);
        };
        let mods = mods::scan(&paths);
        let Ok(settings) = UserSettings::load(&paths) else {
            eprintln!("couldn't read user_settings.json");
            std::process::exit(1);
        };
        let a = loadorder::analyze(&mods, &settings, &cfg.conflict_winners, loadorder::current(&paths).as_ref());
        let missing: Vec<String> = a.missing.keys().cloned().collect();
        if missing.is_empty() {
            println!("No needed mods are missing.");
            return Ok(());
        }
        match needed::find(&missing, &cfg.needed_known, &cfg.needed_wrong) {
            Ok(found) => {
                for f in found {
                    match f.item {
                        Some(i) => println!("  {:<20} → {} ({}, {} subs)", f.namespace, i.title, i.id, i.subscriptions),
                        None => println!("  {:<20} → not found", f.namespace),
                    }
                }
            }
            Err(e) => {
                eprintln!("{e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    if std::env::args().any(|a| a == "--report") {
        print_report();
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ReHearth")
            .with_app_id("rehearth")
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([1060.0, 660.0])
            .with_icon(window_icon()),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native("ReHearth", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
