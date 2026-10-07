# ReHearth

A launcher and mod manager for Stonehearth on Linux. It sorts out mod load order, finds the mod behind an
error, installs Workshop mods without leaving the app, and ships a patch mod that cuts lag in big towns.

> ReHearth is a fan project. It is not affiliated with or endorsed by Radiant Entertainment or Riot Games.
> You need your own copy of Stonehearth on Steam.

## Features

- **Mod load order, fixed for you.** ReHearth reads every mod's manifest and works out the order the
  game will load them in. When you press PLAY it switches on mods your other mods depend on, keeps the
  newest copy of a mod that's installed twice, and switches off mods that can't be read. When two mods
  replace the same file, you pick which copy wins.
- **Doctor.** After each session it reads `stonehearth.log`, groups the errors, names the mod that caused
  each one and explains it in plain words. While the game runs it shows the log live.
  - **Safe mode**: play one session with only the base game.
  - **Find the problem mod**: halves the suspect mods each test session until one is left. It uses launch
    arguments only, so your mod settings never change.
  - Detailed logging for a single mod.
- **Workshop in the app.** Browse and search the whole Stonehearth Workshop, then install and remove mods
  through your running Steam client. No API key, no trip to the Steam overlay.
- **Saves.** Back up saves to `.tar.gz` and restore them (your current saves are backed up first).
- **The ReHearth patch mod** (built on ACE): bug and lag fixes for big towns. Install it from the Home
  screen. See [mods/rehearth_patch](mods/rehearth_patch/README.md).
- **Safe by default.** ReHearth never deletes anything it didn't create, and keeps a backup of your
  `user_settings.json` before it first changes it.

## Getting started

1. Install Stonehearth from Steam and run it once, so it has a settings file.
2. Install ReHearth:

   ```sh
   curl -fsSL https://raw.githubusercontent.com/Aphrodine-wq/rehearth/main/install.sh | sh
   ```

   This puts `rehearth` in `~/.local/bin` and adds **ReHearth** to your app menu. To do it by hand, download
   `rehearth-linux-x86_64.tar.gz` from [Releases](https://github.com/Aphrodine-wq/rehearth/releases),
   extract it and run `./install.sh` inside it.
3. Open **ReHearth** from your app menu (or run `rehearth`).
   - It finds Stonehearth in any of your Steam libraries, including Flatpak Steam. If it doesn't, set the
     game folder in **Settings**.
   - The Home screen shows your mod health. Press **Fix all**, or just press **PLAY**: fixes run then too.
4. To use the lag fixes, subscribe to [ACE](https://steamcommunity.com/sharedfiles/filedetails/?id=1577375188)
   (from the **Workshop** screen works) and install the ReHearth patch from **Home**.

The game always starts through Steam (`steam -applaunch 253250`), so it keeps your Proton settings.

To remove ReHearth: `curl -fsSL https://raw.githubusercontent.com/Aphrodine-wq/rehearth/main/install.sh | sh -s -- --uninstall`.
Your mods, saves and game settings are left alone.

### Requirements

- Linux, x86_64, with glibc 2.35 or newer (Ubuntu 22.04, Fedora 36, Debian 12, Arch, SteamOS and later).
- Steam, native or Flatpak. Workshop installs need Steam running.
- A GPU with Vulkan or OpenGL.

### Screens

| Screen | What it's for |
|---|---|
| **Home** | the game's title art, mod health with **Fix all**, the last session, quick settings |
| **Mods** | health check with one-click fixes, files two mods both replace (pick the winner), every mod in load order with on/off switches |
| **Workshop** | browse, search, install and remove Workshop mods |
| **Saves** | save list, backups, restore |
| **Doctor** | errors from the last session blamed on a mod, the live log, safe mode, find the problem mod |
| **Settings** | auto-fix on launch, game folder, extra launch arguments, graphics |

**PLAY** is at the bottom left on every screen.

### Command line

| Flag | Effect |
|---|---|
| `--report` | print what ReHearth sees: game folder, version, mods in load order, problems |
| `--fix` | apply the automatic mod fixes without opening a window |
| `--browse [search]` | list Workshop mods as text |
| `--tab <name>` | open on a screen (`home`, `mods`, `workshop`, `saves`, `doctor`, `settings`) |
| `--version` | print the version |

### Where things are

| What | Where |
|---|---|
| Mods | `mods/` in the Stonehearth folder (`steamapps/common/Stonehearth`) |
| Workshop mods | `steamapps/workshop/content/253250/` in the same Steam library |
| Game log | `stonehearth.log` in the Stonehearth folder |
| Game settings | `user_settings.json` in the Stonehearth folder (backup: `user_settings.json.rehearth-bak`) |
| Load-order fix mod | `mods/rehearth_load_order/` (generated, safe to delete) |
| ReHearth settings | `~/.config/rehearth/config.json` |
| Cached title art | `~/.cache/rehearth/art/` |

## How mod loading works

From the official modding guide and the engine: base mods load first, and every other mod loads after
the mods in its manifest `dependencies`. A missing or switched-off dependency is silently ignored, mods
in a dependency loop are switched off for the session, and when two mods override the same file the
later one wins. Mods with no dependency between them have no guaranteed order.

That last part is why mod setups break at random. ReHearth fixes it by writing **ReHearth Load Order**
(`mods/rehearth_load_order`): a generated mod that depends on every mod in a file conflict and overrides
each contested file with the chosen copy. It points straight at that mod's file, so nothing is copied.
The pick defaults to the most recently updated mod, and you can change it per file on the **Mods**
screen.

Auto-fix leaves debug-only mods (like Debug Tools) alone, since other mods list them as load-order hints
rather than real dependencies.

## Building from source

You need Rust 1.95 or newer ([rustup](https://rustup.rs)). Nothing else: the fonts and the patch mod are
built into the binary.

```sh
git clone https://github.com/Aphrodine-wq/rehearth
cd rehearth
cargo install --path launcher
```

Run the tests with `cargo test` in `launcher/`.

To make a release, bump `version` in `launcher/Cargo.toml`, commit, and push a matching tag
(`git tag v0.2.0 && git push --tags`). GitHub Actions builds it on Ubuntu 22.04 and publishes it.

## Project layout

| Folder | Contents |
|---|---|
| `launcher/` | the ReHearth app (Rust, egui) |
| `launcher/src/tabs/` | one file per screen |
| `launcher/src/loadorder.rs` | the load-order engine and auto-fix |
| `launcher/src/doctor.rs`, `bisect.rs` | log reading, blame, and find the problem mod |
| `launcher/src/workshop.rs`, `steam.rs` | Workshop browsing, and installs through the Steam client |
| `mods/rehearth_patch/` | the patch mod (built into the launcher) |
| `mods/rehearth_bench/`, `tools/bench.sh` | a scripted big-town benchmark (unfinished: hearthlings in the test world don't haul yet) |
| `packaging/` | the desktop entry |

## Notes for modders

Things learned while building this:

- Never run `steamcmd +login anonymous` against a real Steam library. It shares the library, treats owned
  games as unlicensed, and deletes their files. ReHearth's Workshop installs go through the Steam
  client's own `libsteam_api.so` instead.
- Mods symlinked into `mods/` are invisible to the game under Proton. Copy them.
- `--game.main_mod=<mod>` with `skip_title` hangs on a black screen in 1.1 (microworld too):
  `radiant:new_game` only fires after the title screen asks for a new game.
- `radiant.exit()` is ignored outside autotests.
- `radiant.util.get_config` finds the calling mod from the stack, so calling it through a tail call
  fails; use `get_global_config('mods.<ns>.<key>')`.
- Some base `.smod` files (Northern Alliance, Rayya's Children) contain several `manifest.json` files;
  read the shallowest one.

## Third-party code

ReHearth uses egui/eframe, wgpu, serde, ureq, image and libloading (see `launcher/Cargo.toml`), and
bundles the Google Sans and Google Sans Code fonts under the SIL Open Font License
([OFL.txt](launcher/assets/fonts/OFL.txt)). Parts of the patch mod are derived from
[Stonehearth ACE](https://steamcommunity.com/sharedfiles/filedetails/?id=1577375188), MIT licensed by the Stonehearth ACE Team
([LICENSE.ace.md](mods/rehearth_patch/LICENSE.ace.md)). Release downloads carry these license files in
`licenses/`.

## License

Copyright © 2026 James Walton. ReHearth is released under the [MIT License](LICENSE).
