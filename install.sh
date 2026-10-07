#!/bin/sh
# Installs ReHearth for the current user: the app in ~/.local/bin and a menu entry.
#   curl -fsSL https://raw.githubusercontent.com/Aphrodine-wq/rehearth/main/install.sh | sh
# Run from an extracted release folder, it installs that copy instead of downloading.
# Pass --uninstall to remove it again (your mods, saves and settings are left alone).
# Updating later: ReHearth offers new versions itself, or run `rehearth --update`.
set -eu

REPO="Aphrodine-wq/rehearth"
BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
APP_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
ICON_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/256x256/apps"

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$BIN_DIR/rehearth" "$APP_DIR/rehearth.desktop" "$ICON_DIR/rehearth.png"
    echo "ReHearth removed."
    exit 0
fi

case "$(uname -m)" in
    x86_64) ;;
    *) echo "ReHearth releases are built for x86_64 only. Build from source instead (see the README)." >&2; exit 1 ;;
esac

# when run as a file (not piped from curl), install the copy beside it if there is one
here=""
[ -f "$0" ] && here=$(cd "$(dirname "$0")" && pwd)
tmp=""
if [ -n "$here" ] && [ -f "$here/rehearth" ] && [ -x "$here/rehearth" ]; then
    src="$here"
else
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    url="https://github.com/$REPO/releases/latest/download/rehearth-linux-x86_64.tar.gz"
    echo "Downloading $url"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url" -o "$tmp/rehearth.tar.gz"
    else
        wget -qO "$tmp/rehearth.tar.gz" "$url"
    fi
    tar -xzf "$tmp/rehearth.tar.gz" -C "$tmp"
    src=$(find "$tmp" -mindepth 1 -maxdepth 1 -type d | head -n 1)
fi

mkdir -p "$BIN_DIR" "$APP_DIR" "$ICON_DIR"
install -m 755 "$src/rehearth" "$BIN_DIR/rehearth"
[ -f "$src/rehearth.png" ] && install -m 644 "$src/rehearth.png" "$ICON_DIR/rehearth.png"
sed "s|^Exec=.*|Exec=$BIN_DIR/rehearth|" "$src/rehearth.desktop" > "$APP_DIR/rehearth.desktop"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$APP_DIR" || true

echo "Installed $("$BIN_DIR/rehearth" --version) to $BIN_DIR/rehearth."
echo "Open it from your app menu, or run: rehearth"
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "Note: $BIN_DIR is not on your PATH, so the menu entry is the easy way in." ;;
esac
