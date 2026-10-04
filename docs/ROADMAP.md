# Implementation plan

Order is chosen by: (1) fixes something users hit daily, (2) needs no upstream work, (3) unlocks later phases.
Details and feasibility per feature are in `docs/FEATURE_MATRIX.md`.

## Done
Clipboard (text, images, files both ways), drag & drop, TLS verification + known hosts, in-session toolbar,
Adwaita-dark restyle of manager and viewer.

## P1 – Daily-use essentials (about 3–4 weeks)
1. **Vault** (`nexdesk-vault`): Argon2id (m=64 MiB, t=3, p=1) → 256-bit key wraps a random data key; data encrypted with
   XChaCha20-Poly1305; header carries version, salt, KDF params; atomic write, mode 0600; `Secret` types zeroized.
   First-launch wizard, recovery key (24 words, 3-word verification), idle auto-lock.
   Secret Service backend as the alternative; both implement `CredentialStore`.
2. **Dynamic resize + scaling toggle** via DisplayControl, debounced 150 ms.
3. **Toolbar v2**: Ctrl+Alt+Del, screenshot, pause, scaling, transfer progress bar with 2 px hidden-state strip.
4. **Pre/post-connection commands** (argv-based by default, timeouts, import confirmation).
5. **Zero-trust profile** flag and "TLS/NLA required" flag enforced in the engine.
Exit criteria: all unit tests green; manual test pass on Ubuntu X11 and Wayland against a Windows 10/11 and a Server host.

## P2 – Management and resilience (3–4 weeks)
Groups/search/quick connect/tabs, auto-reconnect, SSH tunnel, performance profiles (text vs media), colour depth and FPS cap,
transfers manager with queue and collision dialog, clipboard history, pre-connection baseline check, GUI e2e harness.

## P3 – Heavy protocol work (6–10 weeks, parts depend on IronRDP)
Drive redirection (RDPDR), multi-monitor, wgpu renderer / HiDPI, download resume, audio, session recording, PAM/polkit unlock.

## P4 – Enterprise (ongoing)
Policy file, audit log, Kerberos/SSO, `.deb` + Flatpak, signed releases, smartcard / RD Gateway when upstream lands.

## Explicit non-goals (see matrix for reasons)
Virtual display driver injection, mounting remote folders in Nautilus, NAT hole-punching relay, clipboard compression, VNC/SSH protocols.

## Risks
* IronRDP features (RDPEGFX, RAIL, gateway, smartcard) are upstream dependencies; we vendor-patch only small hooks.
* gpui tracks Zed `main`: pin the git revision before release.
* Wayland: global shortcut capture and file drops depend on compositor support; documented fallback is `--x11`.
