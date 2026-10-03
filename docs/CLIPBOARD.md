# Clipboard, file copy and drag & drop

## Root cause of the original problem
`ironrdp-client` picks its clipboard backend inside `RdpClient::run`: a real one on Windows, and on every
other OS a `StubClipboard` that does nothing. So on Linux nothing was ever redirected: no text, no files,
in either direction. `vendor/ironrdp-client` now lets the application inject a backend, and the new crate
`nexdesk-clipboard` is that backend.

## What works now
| Direction | Text | Images | Files / folders |
|---|---|---|---|
| Windows -> Linux | yes | yes (DIB -> PNG) | yes, downloaded to a private staging dir, then offered as `text/uri-list` + `x-special/gnome-copied-files` (Nautilus, Dolphin, Nemo, Thunar paste them) |
| Linux -> Windows | yes | yes (PNG -> DIB) | yes, streamed from disk on demand, folders recursive |
| Drop files on the window | - | - | yes (X11 / XWayland): offered to the remote and pasted with Ctrl+V automatically (`--no-drop-paste` to disable) |

Behaviour notes
* Text and images are fetched lazily, only when something pastes.
* File selections up to 64 MiB are downloaded as soon as they are copied (so the paste is instant); larger ones start
  on first paste. Staging lives in `~/.cache/nexdesk/clipboard/p<pid>/` (mode 0700), the last two selections are kept,
  everything is removed on exit and stale directories of crashed sessions are swept at start-up.
* Server-supplied paths are sanitised twice (CLIPRDR layer + `nexdesk-core::clipfiles`); `..`, drive letters, `:` are refused.
* Needs the server to allow it: *Remote Desktop Session Host > Device and Resource Redirection > "Do not allow Clipboard redirection"* must not be enabled.

## X11 and Wayland
The local side speaks the X11 selection protocol (ICCCM, XFixes change notification, INCR for big payloads).
* **X11 session**: native.
* **Wayland session**: through XWayland, which GNOME/Mutter, KDE/KWin and wlroots compositors bridge to the Wayland clipboard in
  both directions. `DISPLAY` must be set (it is on stock Ubuntu).
* Window system can be forced with `--x11` / `--wayland`.

Known limit: the windowing toolkit (winit 0.30) does not deliver *file drops* on native Wayland windows. Run with `--x11`
(the window then runs through XWayland) to use drag & drop; copy/paste works either way. Dragging a file **out of** the remote
desktop onto a local window is not part of the RDP protocol; copy it on the remote and paste in your file manager.

## Tests
`cargo test -p nexdesk-clipboard` runs the engine against a scripted fake desktop; the X11 transport tests (including a 3 MB INCR
transfer) need an X server: `xvfb-run -a cargo test -p nexdesk-clipboard x11 -- --test-threads=1`.
