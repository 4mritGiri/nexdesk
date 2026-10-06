#!/usr/bin/env bash
# Register NexDesk with your desktop (name + icon in the dock/app menu) when you run it from
# the build folder instead of an installed .deb. Per-user, no sudo. Undo: packaging/install-local.sh --remove
set -euo pipefail
cd "$(dirname "$0")/.."
APPS="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
ICONS="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps"
if [ "${1:-}" = "--remove" ]; then
  rm -f "$APPS/nexdesk.desktop" "$ICONS/nexdesk.svg"
  echo "removed"; exit 0
fi
BIN="$(pwd)/target/release/nexdesk"
[ -x "$BIN" ] || { echo "build first: cargo build --release -p nexdesk -p nexdesk-rdp" >&2; exit 1; }
mkdir -p "$APPS" "$ICONS"
install -m644 packaging/nexdesk.svg "$ICONS/nexdesk.svg"
sed "s|^Exec=nexdesk|Exec=$BIN|" packaging/nexdesk.desktop > "$APPS/nexdesk.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database -q "$APPS" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor" || true
echo "installed: start NexDesk from the app menu (log out/in if the dock still says Unknown)"
