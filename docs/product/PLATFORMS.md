# Platform support

Status words follow `docs/product/FEATURE_MATRIX.md`: **verified** = run on a real machine, **builds in CI** = compiled by the CI jobs but not exercised by a person, **planned**.

| | Linux X11 | Linux Wayland | Windows | macOS |
|---|---|---|---|---|
| **Manager (RDP client UI)** | verified | verified | planned (needs a named-pipe replacement for the unix-socket single-instance code, see below) | planned (unix sockets work; GPUI supports Metal; untested) |
| **RDP session window** | verified | verified | planned | planned |
| **Native viewer (`nexdesk-peer-view`)** | verified | verified | builds in CI, untested | builds in CI, untested |
| **Native agent (share this computer)** | verified | built, awaiting real-desktop tests (portal + PipeWire) | planned: Desktop Duplication + SendInput | planned: ScreenCaptureKit + CGEvent |
| **Relay / rendezvous server** | verified | n/a | builds in CI | builds in CI |

## How the code is organised for this

* `crates/nexdesk-peer/src/platform/` holds everything that depends on the operating system and has the same API in each backend:
  `Display` (open the share; on Wayland this shows the desktop's own dialog), `Capture` (next changed tiles, cursor, monitors) and `Injector` (mouse and keyboard).
  `linux/` has the X11 and Wayland backends; `unsupported.rs` is what other systems use today and reports a clear message.
  To add a backend, copy `unsupported.rs`, implement the same API, and select it with `#[cfg(target_os = "...")]` in `platform/mod.rs`. `host.rs` does not change.
* `keymap.rs` maps physical keys to the wire's key numbers (Linux evdev) so a Windows or macOS viewer can control a Linux host.
* `paths.rs` knows where config, downloads and pictures live on each system.
* `clip/` uses the X11 selection protocol on Linux and polls the system clipboard elsewhere.

## What stops the manager and RDP window on Windows

`nexdesk-rdp/src/ipc.rs` and `nexdesk-ui/src/instance.rs` use unix-domain sockets for "one window, many tabs" and focusing the manager. Windows needs named pipes behind the same functions. The keyboard-grab module is already Linux-only and falls back to no grab elsewhere.
After that, GPUI's Windows and macOS backends are used by the manager; the RDP window (winit + softbuffer) is already portable.

## Building

`make build` (this computer), `make deps` (Ubuntu/Debian packages), `make portable` (viewer, agent and relay only, works on Windows, macOS and Linux). CI: `.github/workflows/ci.yml`; releases: `release.yml`.
