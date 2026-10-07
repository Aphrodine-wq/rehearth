# ReHearth

A launcher and mod manager for Stonehearth on Linux, plus a patch mod with lag fixes
for big towns. Built on top of ACE.

- `launcher/`: the ReHearth app (Rust/egui). `cargo install --path launcher`, then run
  `rehearth`. Command-line modes: `--report` (what it sees, load order, issues),
  `--fix` (apply the automatic mod fixes), `--browse [search]` (the Workshop as text),
  `--tab <name>` (open on a screen).
- `mods/rehearth_patch/`: the patch mod. It ships inside the launcher; install it from
  Home or the Workshop screen.
- `mods/rehearth_bench/` and `tools/bench.sh`: a scripted big-town benchmark. Not
  finished: hearthlings in the test world don't haul yet, so it doesn't load the sim.

## Screens

- **Home**: the game's own title art, mod health at a glance with "Fix all", the last
  session, quick settings. PLAY sits at the foot of the menu on every screen.
- **Mods**: a health check with one-click fixes, the files two mods both replace
  (with a picker for which copy wins), and every mod in the order the game will load
  it. On/off switches write `mods.<type>.<namespace>.enabled` in `user_settings.json`,
  the way the game's own mod screen does.
- **Workshop**: browse and search the whole Stonehearth Workshop in-app; install and
  remove through the running Steam client (see `src/steam.rs`).
- **Saves**: save list, tar.gz backups, restore (backs up current saves first).
- **Doctor**: reads `stonehearth.log` after each session. Errors are grouped, blamed on
  a mod (from the traceback, or from which mod replaced the file it failed in),
  explained in plain words, and openable at the failing line. Crashes show the last
  lines of the log. While the game runs it shows the log live. Tools: safe mode (base
  game only for one session), detailed logging per mod (`logging.mods.<ns>.log_level`),
  and "find which mod causes it", which halves the suspects each test session using
  launch arguments only, so your settings never change.
- **Settings**: auto-fix on launch, game folder, extra launch args, graphics.

## How mod loading works (and what auto-fix does)

From the official modding guide and the engine: base mods load first; every other mod
loads after the mods in its manifest `dependencies`. A missing or switched-off
dependency is silently ignored, mods in a dependency loop are switched off for the
session, and when two mods override the same file the later one wins. Mods with no
dependency between them have no guaranteed order.

Auto-fix (on by default, runs when you press Play) switches on installed mods that
your mods build on (debug-only mods like Debug Tools are just load-order hints and
are left alone), keeps the newest copy when a mod is installed twice, switches off
mods whose manifest can't be read, and writes **ReHearth Load Order**
(`mods/rehearth_load_order`): a generated mod that depends on every mod in a file
conflict and overrides each contested file with the chosen copy (pointing straight at
that mod's file, nothing copied). The pick defaults to the most recently updated mod,
and you can change it per file. It never deletes anything.

ReHearth only ever deletes folders carrying its `.rehearth-managed` marker, and keeps a
one-time backup of `user_settings.json` (`user_settings.json.rehearth-bak`).

Google Sans and Google Sans Code are bundled under the SIL Open Font License
(`launcher/assets/fonts/OFL.txt`).

## Things learned the hard way

- Never run `steamcmd +login anonymous` against a real Steam library: it shares the
  library, treats owned games as unlicensed, and deletes their files. Workshop downloads
  go through the Steam client's own `libsteam_api.so` instead.
- Mods symlinked into `mods/` are invisible to the game under Proton. Copy them.
- `--game.main_mod=<mod>` with `skip_title` hangs on a black screen in 1.1 (microworld
  too): `radiant:new_game` only fires after the title screen asks for a new game.
- `radiant.exit()` is ignored outside autotests.
- `radiant.util.get_config` finds the calling mod from the stack, so calling it through
  a tail call fails; use `get_global_config('mods.<ns>.<key>')`.
- Some base .smod files (Northern Alliance, Rayya's Children) contain several
  `manifest.json` files; read the shallowest one.
