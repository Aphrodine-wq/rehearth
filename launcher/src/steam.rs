//! Subscribing to and downloading Workshop items through the Steam client the
//! player already has running, so mods land exactly where Steam (and the game)
//! expect them and keep updating like any other subscription.
//!
//! The Steam API is loaded from the client's own libsteam_api.so and only ever
//! initialised in a short-lived `rehearth --steam-worker` child process: Steam
//! shows "playing Stonehearth" while a download runs and stops the moment it ends.

use std::ffi::{CStr, c_char, c_void};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use libloading::{Library, Symbol};

use crate::game::APP_ID;

// EItemState bits
const SUBSCRIBED: u32 = 1;
const INSTALLED: u32 = 4;
const NEEDS_UPDATE: u32 = 8;
const DOWNLOADING: u32 = 16;
const DOWNLOAD_PENDING: u32 = 32;

/// How long a download may sit without any progress before we give up.
const STALL_TIMEOUT: Duration = Duration::from_secs(120);

fn library_path() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    [".local/share/Steam/steamrt64", ".steam/steam/steamrt64", ".steam/root/steamrt64"]
        .iter()
        .map(|d| home.join(d).join("libsteam_api.so"))
        .find(|p| p.is_file())
}

/// The handful of flat-API calls the worker needs.
struct Api {
    _lib: Library,
    ugc: *mut c_void,
    utils: *mut c_void,
    run_callbacks: unsafe extern "C" fn(),
    shutdown: unsafe extern "C" fn(),
    subscribe: unsafe extern "C" fn(*mut c_void, u64) -> u64,
    unsubscribe: unsafe extern "C" fn(*mut c_void, u64) -> u64,
    download: unsafe extern "C" fn(*mut c_void, u64, bool) -> bool,
    state: unsafe extern "C" fn(*mut c_void, u64) -> u32,
    download_info: unsafe extern "C" fn(*mut c_void, u64, *mut u64, *mut u64) -> bool,
    install_info: unsafe extern "C" fn(*mut c_void, u64, *mut u64, *mut c_char, u32, *mut u32) -> bool,
    call_done: unsafe extern "C" fn(*mut c_void, u64, *mut bool) -> bool,
}

impl Api {
    fn init() -> Result<Self> {
        let path = library_path().context("couldn't find Steam's libsteam_api.so (is Steam installed?)")?;
        // SAFETY: Steam's own library; every symbol below is part of its stable flat C API.
        unsafe {
            let lib = Library::new(&path).with_context(|| format!("couldn't load {}", path.display()))?;
            macro_rules! sym {
                ($name:literal) => {{
                    let s: Symbol<_> = lib.get(concat!($name, "\0").as_bytes()).context(concat!("missing ", $name))?;
                    *s
                }};
            }
            let init: unsafe extern "C" fn(*mut c_char) -> i32 = sym!("SteamAPI_InitFlat");
            let mut err = [0 as c_char; 1024];
            let code = init(err.as_mut_ptr());
            if code != 0 {
                let msg = CStr::from_ptr(err.as_ptr()).to_string_lossy().to_string();
                bail!("Steam isn't available ({}). Is Steam running and logged in?", if msg.is_empty() { format!("code {code}") } else { msg });
            }
            let get_ugc: unsafe extern "C" fn() -> *mut c_void = sym!("SteamAPI_SteamUGC_v021");
            let get_utils: unsafe extern "C" fn() -> *mut c_void = sym!("SteamAPI_SteamUtils_v011");
            let api = Api {
                ugc: get_ugc(),
                utils: get_utils(),
                run_callbacks: sym!("SteamAPI_RunCallbacks"),
                shutdown: sym!("SteamAPI_Shutdown"),
                subscribe: sym!("SteamAPI_ISteamUGC_SubscribeItem"),
                unsubscribe: sym!("SteamAPI_ISteamUGC_UnsubscribeItem"),
                download: sym!("SteamAPI_ISteamUGC_DownloadItem"),
                state: sym!("SteamAPI_ISteamUGC_GetItemState"),
                download_info: sym!("SteamAPI_ISteamUGC_GetItemDownloadInfo"),
                install_info: sym!("SteamAPI_ISteamUGC_GetItemInstallInfo"),
                call_done: sym!("SteamAPI_ISteamUtils_IsAPICallCompleted"),
                _lib: lib,
            };
            if api.ugc.is_null() || api.utils.is_null() {
                (api.shutdown)();
                bail!("Steam didn't provide the Workshop interface");
            }
            Ok(api)
        }
    }

    fn pump(&self) {
        unsafe { (self.run_callbacks)() }
    }

    fn state(&self, id: u64) -> u32 {
        unsafe { (self.state)(self.ugc, id) }
    }

    fn progress(&self, id: u64) -> (u64, u64) {
        let (mut done, mut total) = (0, 0);
        unsafe { (self.download_info)(self.ugc, id, &mut done, &mut total) };
        (done, total)
    }

    fn folder(&self, id: u64) -> Option<String> {
        let mut size = 0u64;
        let mut stamp = 0u32;
        let mut buf = [0 as c_char; 4096];
        let ok = unsafe { (self.install_info)(self.ugc, id, &mut size, buf.as_mut_ptr(), buf.len() as u32, &mut stamp) };
        ok.then(|| unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().to_string())
    }

    /// Pumps callbacks until an async call finishes or the deadline passes.
    fn wait_call(&self, call: u64, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            self.pump();
            let mut failed = false;
            if unsafe { (self.call_done)(self.utils, call, &mut failed) } {
                return !failed;
            }
            thread::sleep(Duration::from_millis(100));
        }
        false
    }
}

impl Drop for Api {
    fn drop(&mut self) {
        unsafe { (self.shutdown)() }
    }
}

fn ready(state: u32) -> bool {
    state & INSTALLED != 0 && state & (NEEDS_UPDATE | DOWNLOADING | DOWNLOAD_PENDING) == 0
}

/// Entry point for `rehearth --steam-worker <subscribe|unsubscribe|state> <id>`.
/// Prints one event per line on stdout for the launcher to follow.
pub fn worker(args: &[String]) -> i32 {
    match run_worker(args) {
        Ok(()) => 0,
        Err(e) => {
            println!("error {e:#}");
            1
        }
    }
}

fn run_worker(args: &[String]) -> Result<()> {
    let (Some(cmd), Some(id)) = (args.first(), args.get(1)) else {
        bail!("usage: --steam-worker <subscribe|unsubscribe|state> <workshop id>");
    };
    let id: u64 = id.parse().context("workshop id must be a number")?;
    let api = Api::init()?;
    match cmd.as_str() {
        "state" => {
            let s = api.state(id);
            let (done, total) = api.progress(id);
            println!("state {id} {s} {done} {total} {}", api.folder(id).unwrap_or_default());
        }
        "subscribe" => {
            if api.state(id) & SUBSCRIBED == 0 {
                let call = unsafe { (api.subscribe)(api.ugc, id) };
                if !api.wait_call(call, Duration::from_secs(30)) || api.state(id) & SUBSCRIBED == 0 {
                    bail!("Steam wouldn't subscribe to this item");
                }
            }
            println!("subscribed {id}");
            unsafe { (api.download)(api.ugc, id, true) };
            let mut last = (0, 0);
            let mut last_change = Instant::now();
            loop {
                api.pump();
                let state = api.state(id);
                if ready(state) {
                    break;
                }
                let now = api.progress(id);
                if now != last {
                    last = now;
                    last_change = Instant::now();
                    println!("progress {id} {} {}", now.0, now.1);
                } else if last_change.elapsed() > STALL_TIMEOUT {
                    bail!("the download stalled; Steam may be busy with another download");
                }
                thread::sleep(Duration::from_millis(250));
            }
            println!("installed {id} {}", api.folder(id).unwrap_or_default());
        }
        "unsubscribe" => {
            let call = unsafe { (api.unsubscribe)(api.ugc, id) };
            if !api.wait_call(call, Duration::from_secs(30)) || api.state(id) & SUBSCRIBED != 0 {
                bail!("Steam wouldn't unsubscribe from this item");
            }
            println!("unsubscribed {id}");
        }
        other => bail!("unknown worker command {other}"),
    }
    Ok(())
}

pub enum Event {
    Subscribed,
    Progress(u64, u64),
    Done,
    Failed(String),
}

/// Runs the worker in a child process and streams its events back.
pub fn spawn(cmd: &'static str, id: u64) -> Result<Receiver<Event>> {
    let exe = std::env::current_exe().context("couldn't find the rehearth executable")?;
    let mut child = Command::new(exe)
        .args(["--steam-worker", cmd, &id.to_string()])
        .env("SteamAppId", APP_ID)
        .env("SteamGameId", APP_ID)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("couldn't start the Steam worker")?;
    let stdout = child.stdout.take().context("no worker output")?;
    let (tx, rx) = channel();
    thread::spawn(move || {
        let mut finished = false;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let event = match parts.as_slice() {
                ["subscribed", ..] => Event::Subscribed,
                ["progress", _, done, total] => Event::Progress(done.parse().unwrap_or(0), total.parse().unwrap_or(0)),
                ["installed", ..] | ["unsubscribed", ..] => Event::Done,
                ["error", ..] => Event::Failed(line.trim_start_matches("error ").to_string()),
                _ => continue, // Steam's own chatter
            };
            finished = matches!(event, Event::Done | Event::Failed(_));
            let _ = tx.send(event);
            if finished {
                break;
            }
        }
        let _ = child.wait();
        if !finished {
            let _ = tx.send(Event::Failed("the Steam worker stopped unexpectedly".into()));
        }
    });
    Ok(rx)
}
