# ReHearth for coding agents

ReHearth can be driven by Claude Code, Codex, Cursor or any other coding agent. Everything the app does
with mods is available as tools: check mod health, apply fixes, find and install missing dependencies,
search and install Workshop mods, read what went wrong in the last game session, and change game settings.

There are two ways in, with the same tools behind both.

## MCP server

`rehearth mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server on stdin/stdout.

Claude Code:

```sh
claude mcp add --scope user rehearth -- rehearth mcp
```

With the AppImage, use its full path instead of `rehearth`:

```sh
claude mcp add --scope user rehearth -- /path/to/ReHearth-x86_64.AppImage mcp
```

Other MCP clients take the same command in their config, for example:

```json
{ "mcpServers": { "rehearth": { "command": "rehearth", "args": ["mcp"] } } }
```

## JSON on the command line

For agents and scripts without MCP, `rehearth api` runs one tool and prints its result as JSON:

```sh
rehearth api                        # list the tools and their arguments
rehearth api status
rehearth api health
rehearth api set_mod_enabled '{"namespace": "trapper_plus", "enabled": false}'
rehearth api last_session '{"max_errors": 5}'
```

Errors come back as `{"error": "..."}` with exit code 1 (2 when the arguments aren't valid JSON).

## Rules the tools follow

- Mods are named by namespace (`stonehearth_ace`), Workshop items by numeric id.
- Tools that change mods or game settings refuse while Stonehearth is running, because the game rewrites
  its settings file when it exits.
- Installs and removals go through the running Steam client and subscribe or unsubscribe the user's Steam
  account. Agents should only do them when the user asked.
- `install_needed_mods` matches missing dependencies to Workshop items by title, then checks each download
  is really the mod that was needed and removes it again if it isn't.
- `launch_game` starts the game. Agents shouldn't call it unless the user asked to play or test.

## Tools

### `status`

Where Stonehearth is, its version, whether it or Steam is running, how many mods are on, and how many problems the health check finds.

No arguments.

### `list_mods`

Every installed mod: namespace, name, source (Built-in, Workshop, Local, ReHearth), on/off, load position, Workshop id, version and dependencies.

Arguments:

- `include_debug`: boolean. Also list debug/test mods. Default false.

### `health`

The mod health check: problems and warnings with suggested fixes, files two mods both replace (and which copy wins), missing dependencies, and the load order.

No arguments.

### `fix_mods`

Applies the safe automatic fixes (the same ones ReHearth runs before Play): switches on mods other mods need, keeps the newest copy of duplicates, switches off unreadable mods, and pins contested files. Never deletes anything.

No arguments.

### `set_mod_enabled`

Switches one mod on or off in the game's user_settings.json.

Arguments:

- `namespace` (required): string
- `enabled` (required): boolean

### `find_needed_mods`

Looks up, on the Workshop, the mods that switched-on mods depend on but that aren't installed. Matches by title, so treat results as candidates. Installs nothing.

No arguments.

### `install_needed_mods`

Finds and installs missing dependencies through Steam, then checks each download really is the needed mod and removes it again if not. Subscribes the user's Steam account; can take minutes.

Arguments:

- `namespaces`: array of string. Only these missing namespaces. Default: all of them.

### `search_workshop`

Searches or browses the Stonehearth Workshop (30 items a page).

Arguments:

- `query`: string. Search text. Empty browses by `sort`.
- `sort`: string (trending, popular, updated, newest)
- `page`: integer

### `workshop_item`

Full details of Workshop items, including the description and whether each is installed.

Arguments:

- `ids` (required): array of integer

### `install_workshop_item`

Subscribes to a Workshop item through Steam and waits until it's downloaded. Returns the mod's namespace.

Arguments:

- `id` (required): integer

### `remove_workshop_item`

Unsubscribes from a Workshop item through Steam, which deletes its files.

Arguments:

- `id` (required): integer

### `last_session`

What happened in the last game session, from stonehearth.log: times, whether it closed cleanly, GPU, whether ReHearth Patch loaded, and errors grouped with the mod that likely caused each and a plain explanation.

Arguments:

- `max_errors`: integer. Default 30.

### `read_log`

The last lines of stonehearth.log.

Arguments:

- `lines`: integer. Default 200.

### `get_game_setting`

Reads a key from the game's user_settings.json, with dots for nesting (e.g. "renderer.draw_distance"). No key returns the whole file.

Arguments:

- `key`: string

### `set_game_setting`

Writes a key in the game's user_settings.json (dots for nesting). ReHearth keeps a backup of the original file.

Arguments:

- `key` (required): string
- `value` (required): any JSON value

### `install_patch`

Installs or updates ReHearth Patch, the lag-fix mod bundled with ReHearth (needs ACE).

No arguments.

### `launch_game`

Starts Stonehearth through Steam. Only call this when the user asked to play or test. `safe` runs the base game only, for this session.

Arguments:

- `mode`: string (normal, safe)
