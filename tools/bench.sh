#!/usr/bin/env bash
# Run one Stonehearth benchmark pass and save the BENCH lines.
#
#   tools/bench.sh <label> [--ace|--no-ace] [--key=value ...]
#
# Extra --key=value args go straight to the game, e.g.
#   tools/bench.sh gc300 --ace --lua.gc_step_pause=300
#   tools/bench.sh quick --mods.rehearth_bench.duration_seconds=60
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
GAME="${STONEHEARTH_DIR:-$HOME/.local/share/Steam/steamapps/common/Stonehearth}"
MODS="$GAME/mods"
ACE_SRC="${ACE_SRC:-$ROOT/vendor/stonehearth_ace_stable}"
RESULTS="$ROOT/results"

label="${1:?usage: bench.sh <label> [--ace|--no-ace] [--key=value ...]}"
shift
ace=keep
game_args=()
for a in "$@"; do
   case "$a" in
      --ace) ace=on ;;
      --no-ace) ace=off ;;
      *) game_args+=("$a") ;;
   esac
done

# Mods are copied in, not symlinked: Proton shows symlinks to the game as reparse
# points and the engine's mod scanner silently skips them. Copies we own carry a
# .rehearth-managed marker so we never touch a mod the player installed.
install_mod() { # <name> <src>
   local dst="$MODS/$1"
   if [[ -e "$dst" && ! -e "$dst/.rehearth-managed" ]]; then
      echo "refusing to overwrite $dst (not installed by rehearth)" >&2
      exit 1
   fi
   mkdir -p "$dst"
   rsync -a --delete --exclude .git --exclude .rehearth-managed "$2/" "$dst/"
   touch "$dst/.rehearth-managed"
}
remove_mod() { # <name>
   local dst="$MODS/$1"
   [[ -e "$dst/.rehearth-managed" ]] && rm -rf "$dst"
   return 0
}

install_mod rehearth_bench "$ROOT/mods/rehearth_bench"
trap 'remove_mod rehearth_bench' EXIT
case "$ace" in
   on) install_mod stonehearth_ace "$ACE_SRC" ;;
   off) remove_mod stonehearth_ace ;;
esac
[[ -e "$MODS/stonehearth_ace" ]] && ace_state=ace || ace_state=vanilla

# game processes, matched on their Windows path so we never match this script
game_pids() { ps -eo pid,args | awk '/[S]:\\steamapps\\common\\Stonehearth\\/ {print $1}'; }
game_running() { [[ -n "$(game_pids)" ]]; }
kill_game() {
   game_pids | xargs -r kill
   sleep 4
   game_pids | xargs -r kill -9
}

pgrep -x steam >/dev/null || { echo "Steam isn't running" >&2; exit 1; }
if game_running; then
   echo "Stonehearth is already running" >&2
   exit 1
fi

mkdir -p "$RESULTS"
stamp="$(date +%Y%m%d-%H%M%S)"
out="$RESULTS/$stamp-$label"

echo "bench: $label ($ace_state) ${game_args[*]:-}"
# the game rewrites its log on start, but clear it first so a previous run's
# summary can't end this one early
rm -f "$GAME/stonehearth.log"
steam -applaunch 253250 \
   --game.main_mod=rehearth_bench \
   --mods.directory.rehearth_bench.enabled=true \
   --mods.rehearth_bench.label="$label" \
   "${game_args[@]}" >/dev/null 2>&1 &

# wait for the game to come up, then for it to exit on its own
for _ in $(seq 120); do
   game_running && break
   sleep 1
done
game_running || { echo "game never started" >&2; exit 1; }

boot_limit="${BENCH_BOOT_TIMEOUT:-120}"
limit="${BENCH_TIMEOUT:-900}"
start=$SECONDS
while game_running; do
   # scripts can't quit the game outside autotests, so close it once the summary is in
   if grep -qa 'BENCH summary' "$GAME/stonehearth.log"; then
      sleep 2
      kill_game
      break
   fi
   if (( SECONDS - start > boot_limit )) && ! grep -qa 'BENCH started' "$GAME/stonehearth.log"; then
      echo "bench never started after ${boot_limit}s; closing the game" >&2
      kill_game
      break
   fi
   if (( SECONDS - start > limit )); then
      echo "timed out after ${limit}s; closing the game" >&2
      kill_game
      break
   fi
   sleep 2
done
sleep 2

cp "$GAME/stonehearth.log" "$out.log"
{
   echo "# $label $ace_state ${game_args[*]:-}"
   grep -a 'BENCH' "$out.log" | sed -E 's/^.*BENCH /BENCH /' || true
} >"$out.txt"

grep -a 'BENCH summary' "$out.txt" || {
   echo "no summary; last log lines:" >&2
   tail -n 25 "$out.log" >&2
   exit 1
}
