# Implementation plan

Order is chosen by: (1) fixes something users hit daily, (2) needs no upstream work, (3) unlocks later phases.
Details and feasibility per feature are in `docs/FEATURE_MATRIX.md`.

## Done
Sidebar: Devices, Address Books, Logs (Connection, File, Alarm, Console). Follow-ups: live log tail without Refresh, export logs (CSV), per-transfer rows with byte counts and speed, online/latency probe on Devices, address-book import/export, tags.
Manager header bar (sidebar toggle, back/forward, search, grid/list), session header bar, speed presets (LAN / Balanced / Slow network).
Clipboard (text, images, files both ways), drag & drop, TLS verification + known hosts, in-session toolbar,
Adwaita-dark restyle of manager and viewer.

## P1 – Daily-use essentials (about 3–4 weeks)
1. **Vault** (DONE: crypto, recovery key, UI; remaining: idle auto-lock, first-run wizard, Secret Service backend) (`nexdesk-vault`): Argon2id (m=64 MiB, t=3, p=1) → 256-bit key wraps a random data key; data encrypted with
   XChaCha20-Poly1305; header carries version, salt, KDF params; atomic write, mode 0600; `Secret` types zeroized.
   First-launch wizard, recovery key (24 words, 3-word verification), idle auto-lock.
   Secret Service backend as the alternative; both implement `CredentialStore`.
2. **Dynamic resize + scaling toggle** via DisplayControl, debounced 150 ms.
3. **Toolbar v2**: Ctrl+Alt+Del, screenshot, pause, 1:1 scaling are **done**; transfer progress bar with 2 px hidden-state strip remains.
4. ~~**Pre/post-connection commands**~~ **Done**: argv-based, 20 s timeout, ignored in imported `.rdp` files.
5. **Zero-trust profile** flag and "TLS/NLA required" flag enforced in the engine.
Exit criteria: all unit tests green; manual test pass on Ubuntu X11 and Wayland against a Windows 10/11 and a Server host.

## P2 – Management and resilience (3–4 weeks)
Groups/quick connect/tabs, auto-reconnect, SSH tunnel, **VPN control** (NetworkManager profile picker in the connection editor, status "VPN connecting → Connected → RDP", wait-for-reachability, optional bring-down after the session; not urgent), performance profiles (text vs media), colour depth and FPS cap,
SSH tunnel for RDP (also the post-quantum path for RDP, see `docs/SECURITY_DESIGN.md`), transfers manager with queue and collision dialog, clipboard history, pre-connection baseline check, GUI e2e harness.

## P3 – Heavy protocol work (6–10 weeks, parts depend on IronRDP)
Drive redirection (RDPDR), multi-monitor, wgpu renderer / HiDPI, download resume, audio, session recording, PAM/polkit unlock.

## P4 – Enterprise (ongoing)
Policy file, audit log, Kerberos/SSO, `.deb` + Flatpak, signed releases, smartcard / RD Gateway when upstream lands.

## P5 – NexDesk native remote control (months; a second product, see `docs/REMOTE.md`)
Own host agent + protocol + rendezvous/relay server so Linux/macOS machines, machines behind NAT and Windows Home can be controlled by ID.
* **P5a** direct IP, Linux X11 host agent, viewer engine, pinned-key handshake, accept prompt, clipboard, software codec. **Done so far (prototype, `docs/PEER.md`)**: agent + viewer over the hybrid handshake, X11 capture with tile diff + LZ4, mouse/keyboard/wheel via XTEST, consent/allow-list/view-only, pinning, end-to-end test on Xvfb. **Left**: clipboard, cursor shape, resize, reconnect, manager UI entry, encrypted identity store, XShm/video codec.
* **P5b** Wayland host (portal/PipeWire/libei), Windows host, audio, files, chat, multi-monitor, recording, TCP tunnels, unattended service.
* **P5c** `nexdesk-hbb` rendezvous + relay, IDs, NAT hole punching, 2FA, self-hosting docs, shared address book.
* **P5d** Windows login screen/UAC service, privacy mode, hardware codecs, macOS host.
* **P6** mobile and web clients, enterprise server tier (OIDC/LDAP, web console, audit).
Security design (hybrid post-quantum handshake X25519+ML-KEM, dual signatures, consent-by-default, audit, supply chain): `docs/SECURITY_DESIGN.md`. The crypto crate (`crates/nexdesk-crypto`, see `docs/CRYPTO.md`) is written first and reviewed before anything else in P5. **Step 1 done (prototype, unreviewed)**: hybrid handshake + record layer + tests.
Prerequisite decision: licence (permissive vs AGPL) and the own-code rule (do not copy code from other remote-desktop projects).

## Explicit non-goals (see matrix for reasons)
Virtual display driver injection, mounting remote folders in Nautilus, clipboard compression, VNC protocol, interoperating with other remote-desktop products' clients and servers. (NAT hole-punching and a relay moved into P5c; SSH tunnels for RDP stay in P2.)

## Risks
* IronRDP features (RDPEGFX, RAIL, gateway, smartcard) are upstream dependencies; we vendor-patch only small hooks.
* gpui tracks Zed `main`: pin the git revision before release.
* Wayland: global shortcut capture and file drops depend on compositor support; documented fallback is `--x11`.
