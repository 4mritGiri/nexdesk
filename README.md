# NexDesk

NexDesk is an open-source (MIT OR Apache-2.0) remote-access suite written in Rust: an RDP client built on IronRDP with a GPUI desktop manager, a native remote-control stack (agent, viewer, self-hosted relay) and, next, a terminal with SSH. The goal is one free, auditable tool for remote desktop, remote support, a multi-protocol connection manager and a terminal on Linux, Windows and macOS. See `docs/product/VISION.md`.

## Workspace

```text
apps/       manager (GPUI window), rdp-viewer (RDP session window)
crates/     core, session, renderer, ui, clipboard, crypto, network (relay), peer (native remote control)
vendor/     patched IronRDP crates
docs/       product/, architecture/, security/, adr/
```
Full tree and the rules behind it: `docs/architecture/ARCHITECTURE.md`.

### Security boundaries

- Saved `.rdp` profiles never persist passwords.
- Passwords are represented by an opaque `Secret` and are not `Debug` printable.
- The manager passes a password to the RDP child through `NEXDESK_PASSWORD`, never a command-line argument.
- Remote clipboard file names are sanitized before staging.
- RDP protocol execution is isolated in `nexdesk-rdp`; the manager owns lifecycle state.
- Server certificates are verified by fingerprint pinning (trust on first use); see `--tls`.
- Before/after-connection commands run without a shell, never see the password, and are ignored in imported `.rdp` files.

## Build

Needs a current stable Rust toolchain (<https://rustup.rs>) and the Linux development packages GPUI uses. On Ubuntu/Debian:

```bash
sudo apt install build-essential pkg-config clang cmake git \
  libpipewire-0.3-dev clang libclang-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-dev libx11-xcb-dev libxcb1-dev libxi-dev \
  libfontconfig-dev libvulkan-dev libssl-dev libzstd-dev
```

(The exact list can vary with the GPUI revision; if the linker reports a missing `-lfoo`, install the matching `libfoo-dev`.)

```bash
cargo test --workspace --exclude nexdesk --exclude nexdesk-ui   # logic tests, no GUI libraries needed
cargo build --release -p nexdesk -p nexdesk-rdp                 # the two executables
```

The first build downloads and compiles GPUI and takes a while.

## Build a .deb and test it on another laptop

On the build machine (Ubuntu/Debian, same CPU architecture as the target laptop):

```bash
cargo build --release -p nexdesk -p nexdesk-rdp
packaging/make-deb.sh            # -> dist/nexdesk_<version>_<arch>.deb
```

`make-deb.sh` only needs `dpkg-deb`. It installs `nexdesk` and `nexdesk-rdp` side by side in `/usr/bin` (the manager looks for the engine next to itself), plus the menu entry and icon.

On the other laptop:

```bash
scp dist/nexdesk_0.2.0_amd64.deb user@laptop:/tmp/      # or copy it with a USB stick
sudo apt install /tmp/nexdesk_0.2.0_amd64.deb           # apt resolves the runtime libraries
nexdesk                                                 # or start "NexDesk" from the app menu
sudo apt remove nexdesk                                 # uninstall (your ~/.config/nexdesk data stays)
```

Runtime needs: a Vulkan-capable GPU/driver (`mesa-vulkan-drivers` is recommended automatically), and for the clipboard/drag & drop on Wayland, XWayland (`DISPLAY` set; default on Ubuntu).
The package is not signed. To publish it for others, upload the `.deb` to a GitHub Release, or put it in an apt repository (`reprepro` / `aptly`) and sign that repository.

Running from the build folder instead of a package? Run `packaging/install-local.sh` once so the dock shows the NexDesk name and icon instead of "Unknown" (the windows identify themselves as `nexdesk`, which must match an installed `nexdesk.desktop`). It is per-user; `--remove` undoes it.

Quick test without installing a package: copy `target/release/nexdesk` and `target/release/nexdesk-rdp` into the same folder on the other laptop (same architecture and similar Ubuntu version, or the libraries from the list above installed) and run `./nexdesk`.

First-run checklist on the test laptop:
1. Add a connection (host, user), connect, and accept the certificate fingerprint once.
2. Check the toolbar in full screen (`Ctrl+Alt+Break`, or the green dot): Ctrl+Alt+Del, screenshot (saved to `~/Pictures`), fit / 1:1, pause.
3. Copy text and a file both ways, and drop a file on the window (`--x11` if you are on Wayland).
4. Look at Logs > Connection / Console if anything fails.

## Running

Build both `nexdesk` and `nexdesk-rdp` so the manager can find the engine beside its executable:

```bash
cargo build --release -p nexdesk -p nexdesk-rdp
./target/release/nexdesk
```

For direct protocol testing:

```bash
NEXDESK_PASSWORD='...' ./target/release/nexdesk-rdp --host jump.example.com -u alice
```

## Migration status

The original `myrdp` core/viewer/app crates were converted into the NexDesk crate names rather than discarded. The existing IronRDP connection, keyboard, mouse, scaling and clipboard behavior remains in `nexdesk-rdp`/`nexdesk-core`.

The new session layer is deliberately a foundation: reconnect policy and OS keyring storage are exposed as boundaries so they can be added without coupling the GPUI UI to IronRDP internals.

## Viewer features added in this revision
* **Full-screen connection bar** (like mstsc): move the mouse to the top edge; pin / minimise / restore / close. `Ctrl+Alt+Break` still toggles full screen.
* **Clipboard**: text, images and files in both directions, X11 and Wayland (XWayland). See `docs/architecture/CLIPBOARD.md`.
* **Drag & drop** local files onto the window (use `--x11` on Wayland).
* **TLS verification**: `--tls ask|accept-new|strict|insecure`, pins fingerprints in `~/.config/nexdesk/known_hosts`, `--forget-host` to reset.
* Connection errors are shown inside the window instead of only on stderr.

Local patches to IronRDP are in `vendor/` (see `vendor/README.md`).

## Session window and speed presets
* Windowed sessions have NexDesk's own header (red/yellow/green dots, title, drag to move, double-click to maximise, drag edges to resize); `--native-frame` uses the system title bar. Full screen shows the floating bar at the top edge.
* `--perf lan|balanced|slow` (or the "Connection speed" choice in the connection editor) controls how much visual decoration the server sends; use *Slow network* over VPN/mobile links.
* Manager: sidebar toggle button (icons only when collapsed), back/forward, search, grid/list view, double-click a connection to connect.

## Devices, Address Books, Logs
* **Devices**: one row per computer with last activity, session count and certificate status; *Forget key* re-asks about the certificate on the next connection.
* **Address Books**: group connections (create a book, click `+ name` to add, Remove, Delete book).
* **Logs**: Connection, File, Alarm and Console pages under the sidebar's Logs group, stored in `~/.local/share/nexdesk/logs/`. Passwords and file contents are never written.

## Themes and preferences
Header "⋯" menu > Preferences (or sidebar > Settings): theme (Midnight / Graphite / Light), sidebar and view defaults, default connection speed, session window options, certificate policy. Stored in `~/.config/nexdesk/settings`.

## Password vault
Preferences > Password vault > *Set up vault*. Passwords are saved only if you tick "Save this password in the vault" when connecting; connections with a saved password then connect without asking. File: `~/.config/nexdesk/vault` (Argon2id + XChaCha20-Poly1305, mode 0600). Keep the recovery key offline: without it and the master password the passwords are unrecoverable by design.

## Session toolbar
Always visible as the header in a window, and as the pill at the top edge in full screen. Right side: **DEL** sends Ctrl+Alt+Del, the camera saves a PNG of the remote screen to your Pictures folder, the corner icon switches between *fit to window* and *actual size* (1:1; push the pointer to a window edge to scroll), and pause freezes the picture and stops sending input. Hover a button for its name. The connection speed preset is applied when connecting and cannot be changed mid-session.

## Before / after connection commands
In the connection editor, *Before connect* (for example `nmcli con up "Work VPN"`) runs before the session starts; if it fails or takes longer than 20 s the connection is not started. *After disconnect* runs when the session ends. They are a program plus arguments, not shell text (`;`, `|`, `$VAR` are literal; use `sh -c '...'` yourself if you really want a shell). They get `NEXDESK_HOST`, `NEXDESK_PROFILE`, `NEXDESK_USER` in the environment, never the password. A `.rdp` file you import cannot carry commands.

## Find computers on your network
Devices > *Scan network* probes the local subnet (at most one /24 per network interface) for TCP port 3389 and lists the computers that answer, with their network name when available. *Add connection* pre-fills the host; *Connect* appears if you already have a connection for it. Nothing is sent after the port answers, and nothing runs unless you press the button. If your friend's PC is not listed: it must be awake and on the same network, Remote Desktop must be enabled (Windows Pro/Enterprise/Server only, Windows Home cannot host RDP) and port 3389 must be allowed in Windows Defender Firewall.

## Connection errors
Failures now show the underlying cause (refused / timed out / unreachable / sign-in rejected) plus a hint, and the same text goes to Logs > Console.

## Roadmap
`docs/product/ROADMAP.md` (phases), `docs/product/FEATURE_MATRIX.md` (status of every feature) and `docs/architecture/REMOTE.md` (the native remote-control mode).

## Remote control of Linux machines (prototype)
`nexdesk-agent` shares an X11 screen; `nexdesk-peer-view HOST:PORT` shows and controls it, over a hybrid post-quantum authenticated channel with a consent prompt. See `docs/architecture/PEER.md` (how to try it, what is enforced, what is missing) and `docs/security/CRYPTO.md`. To reach computers behind NAT by a nine digit ID, run `nexdesk-relay` on a server both sides can reach (see `docs/architecture/RELAY.md`). Tests: `xvfb-run -a cargo test -p nexdesk-peer -p nexdesk-network`.

In the manager, open **Remote Control** to connect to or share a computer without a terminal; text clipboard and the pointer image are synchronised too.

## Platforms

Linux (X11 and Wayland) is the primary platform; the viewer, relay and libraries build on Windows and macOS in CI. Details and what is still missing: `docs/product/PLATFORMS.md`. Code layout: `docs/architecture/ARCHITECTURE.md`.

## Licence

Dual licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Contributions are accepted under the same terms.
NexDesk is original code: please do not submit code copied from other remote-desktop projects.
