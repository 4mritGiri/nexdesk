# NexDesk peer protocol (P5a prototype): agent + viewer

Control a Linux **X11** machine from another Linux machine, directly by IP address, with the hybrid post-quantum handshake from `docs/CRYPTO.md`.
No server is involved yet (ID/relay come in P5c). **Prototype: unreviewed crypto, no Wayland, no Windows host, no audio/files/clipboard yet.**

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
* Wire format: `Msg` in `src/wire.rs` (hello, tile, bye, mouse move/button/wheel, key, ping/pong).

## Tests
`xvfb-run -a cargo test -p nexdesk-peer` starts a real X server and checks: the screen arrives pixel-exact, later changes arrive, mouse moves the real pointer, an unapproved viewer receives nothing, a wrong pin aborts the handshake, view-only blocks input, plus unit tests for diffing, tile bounds and hostile messages.

## Not done yet
Wayland host, Windows/macOS host, clipboard, files, audio, multi-monitor, cursor shape, resize handling (the agent ends the session if the screen size changes), reconnect, a GUI entry in the manager, encrypted identity storage, rate limiting beyond the delay, fuzzing.
