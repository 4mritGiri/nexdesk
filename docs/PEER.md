# NexDesk peer protocol (P5a prototype): agent + viewer

Control a Linux **X11** machine from another Linux machine, directly by IP address, with the hybrid post-quantum handshake from `docs/CRYPTO.md`.
Direct by IP, or by ID through a relay you run (`docs/RELAY.md`); no other server is involved. **Prototype: unreviewed crypto, no Wayland, no Windows host, no audio/files yet.**

## Try it (two terminals, or two laptops)
Build: `cargo build --release -p nexdesk-peer` (produces `nexdesk-agent` and `nexdesk-peer-view`).

On the machine to be controlled (must be an X11 session; on Ubuntu choose "Ubuntu on Xorg" at the login screen):
```bash
nexdesk-agent --listen 0.0.0.0:21118        # LAN; default is 127.0.0.1 (this computer only)
# prints:  agent identity: SHA256:xxxxxxxx-xxxxxxxx-...
```
On the other machine:
```bash
nexdesk-peer-view 192.168.1.50:21118
```
1. The viewer prints the agent's fingerprint and asks `Trust this agent? [y/N]`. Compare it with the one the agent printed (read it over the phone, or in person). It is then pinned in `~/.config/nexdesk/peer/known_agents`; a different identity later is refused (`--forget` resets).
2. The agent prints the viewer's fingerprint and asks `Accept? [y/N]`. Nothing is shared until you answer y. `--allow SHA256:...` pre-approves a viewer; `--no-prompt` denies everybody else; `--view-only` ignores the viewer's mouse and keyboard.
3. A window opens with the remote screen. Mouse, wheel and keys are forwarded (keys as Linux evdev codes, so layouts match when both sides are Linux).

Identities are created on first use in `~/.config/nexdesk/peer/` (files 0600, directory 0700). Prototype: stored unencrypted; they move into the vault later.

## From the manager (no terminal)
Open **Remote Control** in the sidebar.
* **Connect to a NexDesk computer:** enter the agent's address. The manager first asks the agent for its fingerprint (`nexdesk-peer-view --probe`), shows it, and only after you press *Trust and connect* starts the viewer with `--trust <that exact fingerprint>`. Already-pinned agents connect directly; a changed identity is refused.
* **Share this computer:** *Start sharing* runs `nexdesk-agent --stdio-control` as a child process. Each viewer produces an *Allow remote control?* dialog with its fingerprint; nothing is sent until you press Allow. *View only* and *Clipboard* are toggles (set them before starting). The agent stops when you press *Stop sharing* or close the manager.
* The agent and viewer binaries are looked up next to the manager, then on `PATH` (the .deb installs them together).

## Clipboard and cursor
* Text clipboard is synchronised both ways (UTF-8, at most 1 MiB per copy) through the same encrypted channel. It is off for view-only sessions, `--no-clipboard` disables it on either side, and clipboard contents are never logged. Images and files are not synchronised yet.
* The agent sends the real pointer image (XFixes), and the viewer shows it as its cursor over the remote screen.

## Viewer window
Move the pointer to the top edge of the window (or press a shortcut) to show the toolbar; it hides again after a moment.
* **1:1 / Fit:** fit scales the remote screen to the window; 1:1 shows real pixels, and pointing at a window edge scrolls when the remote screen is bigger.
* **Full / Window:** full screen. Shortcut **Ctrl+Alt+Pause**.
* **Screen n/m:** appears when the remote computer has several monitors; each press shows the next one. The mouse is mapped to the chosen monitor. If monitors are plugged in or out, the viewer is told and starts a fresh picture.
* **Ctrl+Alt+Del:** sends that key combination to the remote computer (hidden in view-only sessions). Shortcut **Ctrl+Alt+End**. The shortcut keys themselves are not forwarded.
* **Shot:** saves the remote picture as a PNG in your Pictures folder; a message at the bottom shows the path. Only your local copy is written.

Speed: the agent uses XDamage, so a screen that does not change costs almost nothing (it still checks once a second as a safety net). Changed areas are sent as LZ4-compressed 64x64 tiles. Shared-memory capture and a video codec are still future steps.

## Resize and reconnect
* If the shared screen changes size (resolution change, monitor plugged in), the agent sends a new `Hello` and the viewer starts a fresh picture. The session continues.
* If the connection drops, the viewer window stays open, shows "reconnecting" in the title and retries for about 90 s (1, 2, 4, 8 s steps) to the **same pinned agent identity**; a different identity is never accepted. Queued mouse and key events are discarded on reconnect, so no stale key presses are replayed.
* The agent asks its user again on a new connection, except that the same viewer identity, approved a moment ago, may come back without a new question for 60 s (`--reconnect-grace SECONDS`, `0` = always ask). The handshake still proves the identity. A connection that ends before the agent's first `Hello` (denied or refused) or with a `Bye` is final, so the host user is never nagged by a retry loop.
* A viewer whose network died silently can be refused for up to 45 s, until the agent notices the dead connection; the retry loop covers this.

## What is enforced
* Handshake first, then nothing but authenticated, encrypted records; before authentication only 16 KB frames are read, with a 10 s timeout.
* One viewer at a time; others are dropped. Failed handshakes cost the attacker a 0.5 s delay.
* Viewer checks the agent's pinned identity **before** revealing its own; agent asks consent (or allow-list) **before** sending a single pixel.
* All message sizes and tile bounds are validated before allocation; decompressed tile size must match exactly.
* The agent never logs screen contents or keystrokes; logs show only fingerprints and addresses.
* Idle peers are dropped after 45 s (viewer pings every 10 s).

## Design notes
* Capture: X11 `GetImage` of the root window every ~33 ms, diffed in 64x64 tiles, changed tiles merged per row, LZ4-compressed. Simple and correct; XShm/XDamage and a real video codec (VP9/H.264) are the next performance steps.
* Input: XTEST (`FakeInput`). Wheel = X buttons 4-7.
* Wire format: `Msg` in `src/wire.rs` (hello, tile, cursor, clip, bye, mouse move/button/wheel, key, ping/pong).

## File transfer and chat
* Upload only (viewer to the sharing computer): drop files or folders on the viewer window. The agent's user must accept every transfer (60 s, then it counts as no); view-only sessions refuse all transfers.
* The receiver saves under `Downloads/NexDesk`, never overwrites (`name (1).ext`), never follows a symlink out of that folder, rejects absolute paths, `..`, backslashes and control characters, enforces the announced sizes exactly, writes `.part` files with mode 0600 and deletes them if the connection drops. Limits: 10,000 files, 8 GiB per file, 64 GiB per transfer.
* Flow control: 48 KiB chunks, at most 4 MiB unacknowledged. File names and contents are never logged.
* Chat: both ways, toolbar button or Ctrl+Alt+C in the viewer (typing goes to the chat while it is open, not to the remote computer); in the manager on the Remote Control page. Lines are limited to 2000 bytes, control characters removed, never logged.
* Manager protocol (`--stdio-control`): out `FILES <n> <bytes> <first name>`, `CHAT <text>`; in `files yes|no`, `chat <text>`.
* Not yet: downloading from the remote computer, a file manager, resume.

## Tests
`xvfb-run -a cargo test -p nexdesk-peer` starts a real X server and checks: the screen arrives pixel-exact, later changes arrive, mouse moves the real pointer, an unapproved viewer receives nothing, a wrong pin aborts the handshake, view-only blocks input, clipboard text both ways, the pointer image, the manager flow (probe, `--trust`, consent over stdin/stdout, deny ends the viewer), a screen resize that keeps the session, two monitors with switching and pointer mapping, an idle screen that stops producing tiles, toolbar layout and hit tests, a viewer that survives an agent restart, a folder upload with decline, no-overwrite and hostile-path refusal plus chat both ways, plus unit tests for diffing, tile bounds and hostile messages.

## Not done yet
Wayland host, Windows/macOS host, image/file clipboard, file download and a file manager, audio, a GUI viewer inside the manager window (the viewer is still its own window), encrypted identity storage, rate limiting beyond the delay, fuzzing.
