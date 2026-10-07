#!/usr/bin/env bash
# Build a .deb from already-built release binaries, using only dpkg-deb (no cargo-deb needed).
#   cargo build --release -p nexdesk -p nexdesk-rdp -p nexdesk-peer   (peer tools are optional)
#   packaging/make-deb.sh            ->  dist/nexdesk_<version>_<arch>.deb
# Env: BIN_DIR (default target/release), OUT_DIR (default dist), DEB_ARCH (default dpkg's arch)
set -euo pipefail
cd "$(dirname "$0")/.."
BIN_DIR="${BIN_DIR:-target/release}"
OUT_DIR="${OUT_DIR:-dist}"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
ARCH="${DEB_ARCH:-$(dpkg --print-architecture)}"
for f in nexdesk nexdesk-rdp; do
  [ -x "$BIN_DIR/$f" ] || { echo "missing $BIN_DIR/$f - run: cargo build --release -p nexdesk -p nexdesk-rdp" >&2; exit 1; }
done
ROOT="$(mktemp -d)"; trap 'rm -rf "$ROOT"' EXIT
install -Dm755 "$BIN_DIR/nexdesk"     "$ROOT/usr/bin/nexdesk"
install -Dm755 "$BIN_DIR/nexdesk-rdp" "$ROOT/usr/bin/nexdesk-rdp"
for f in nexdesk-agent nexdesk-peer-view; do [ -x "$BIN_DIR/$f" ] && install -Dm755 "$BIN_DIR/$f" "$ROOT/usr/bin/$f"; done
install -Dm644 packaging/nexdesk.desktop "$ROOT/usr/share/applications/nexdesk.desktop"
install -Dm644 packaging/nexdesk.svg "$ROOT/usr/share/icons/hicolor/scalable/apps/nexdesk.svg"
install -Dm644 README.md "$ROOT/usr/share/doc/nexdesk/README.md"
SIZE="$(du -sk "$ROOT" | cut -f1)"
mkdir -p "$ROOT/DEBIAN"
cat > "$ROOT/DEBIAN/control" <<CTL
Package: nexdesk
Version: $VERSION
Section: net
Priority: optional
Architecture: $ARCH
Installed-Size: $SIZE
Depends: libc6, libxkbcommon0, libxkbcommon-x11-0, libx11-6, libxcb1, libxi6, libwayland-client0, libfontconfig1, libvulkan1
Recommends: mesa-vulkan-drivers
Maintainer: NexDesk <nexdesk@localhost>
Description: NexDesk RDP connection manager
 RDP client with connection manager, clipboard/file redirection,
 certificate pinning and an encrypted password vault.
CTL
cat > "$ROOT/DEBIAN/postinst" <<'PST'
#!/bin/sh
set -e
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q /usr/share/applications || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t /usr/share/icons/hicolor || true
exit 0
PST
chmod 755 "$ROOT/DEBIAN/postinst"
mkdir -p "$OUT_DIR"
dpkg-deb --root-owner-group --build "$ROOT" "$OUT_DIR/nexdesk_${VERSION}_${ARCH}.deb" >/dev/null
echo "built $OUT_DIR/nexdesk_${VERSION}_${ARCH}.deb"
