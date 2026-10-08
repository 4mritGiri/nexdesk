# NexDesk feature matrix

Status legend
* **Done** – implemented and covered by tests or verified on a real machine.
* **Done (unverified)** – implemented, compiles, not yet exercised against a real server / display.
* **Planned P1/P2/P3/P4** – scheduled in `docs/ROADMAP.md` (phase number).
* **Blocked** – needs something outside this repo (IronRDP feature, server component, infrastructure).
* **Out of scope** – deliberately not built (reason given).

Feasibility column: **Easy** (days, client only) · **Medium** (1–3 weeks) · **Hard** (a month+, or protocol work) · **Infra** (needs a server we would have to run).

## 1. What exists today

| Area | Remmina | NexDesk | Notes |
|---|---|---|---|
| Saved connections (.rdp compatible) | yes | **Done** | New / Edit / Duplicate / Delete, `.rdp` import |
| Manager UI in GNOME Files / macOS style | n/a | **Done (unverified)** | rounded window, red/yellow/green dots, header bar with sidebar toggle, back/forward, path pill, search, grid/list switch; collapsible icon sidebar; double-click to connect. Compile-checked only; not rendered here |
| Themes: Midnight (deep navy, default), Graphite (GNOME Files grey), Light; vector icons | n/a | **Done (unverified UI)** | switch live in Preferences; icons are embedded SVGs tinted per theme, so they stay readable on dark and light |
| Preferences page + header "⋯" menu (Preferences, Keyboard Shortcuts, About) | yes | **Done (unverified UI)** | saved in `~/.config/nexdesk/settings`: theme, sidebar, default speed, full screen, key capture, drop auto-paste, system title bar, certificate policy. Applies to the next session |
| Logs: Connection / File / Alarm / Console (sidebar group) | partial | **Done** | `~/.local/share/nexdesk/logs/*.log`, 2 MiB rotation, mode 0600. Connection = started/connected/failed/ended with duration; File = copies offered/downloaded/failed; Alarm = untrusted or changed certificate, user decisions, insecure mode; Console = engine output. No passwords, clipboard text or file contents are ever logged. Refresh/Clear, search filter |
| Devices page (per computer: last activity, session count, certificate pinned, Connect, Forget key) | no | **Done (unverified UI)** | derived from saved connections plus the connection log; "online" probing is **Planned P2** |
| Address Books (named groups of connections: create, add, remove, delete, connect) | no | **Done (unverified UI)** | local file `~/.config/nexdesk/addressbooks`; renames/deletes of connections keep books consistent. Sharing/sync between users needs a server: out of scope for now |
| Session window header bar (same dots, drag, double-click maximise, edge resize, pointer cursors) | no | **Done (unverified)** | windowed mode draws its own 40 px header; `--native-frame` restores the system title bar. Full screen keeps the auto-hide pill |
| Connection speed preset (LAN / Balanced / Slow network) | yes | **Done** | per connection, saved as mstsc `connection type`. Balanced: no wallpaper, menu animations, full-window drag. Slow: also no themes/cursor effects and 16-bit colour (about half the bitmap data) |
| Process isolation per session | no | **Done** | engine crash cannot take down the manager |
| Passwords never on disk, zeroized | keyring | **Done** | handed over by environment, not argv |
| Full screen + `Ctrl+Alt+Break` | yes | **Done** | |
| In-session floating toolbar (min / restore / close / pin) | yes | **Done** | Adwaita-style pill, auto-hide, pin |
| Super / Alt+Tab to remote (X11 + Wayland) | partial | **Done (unverified)** | XInput2 grab / shortcuts-inhibit |
| Server cursor shapes | yes | **Done** | |
| Clipboard text + images | yes | **Done** | confirmed by user, both directions |
| Clipboard files and folders, both directions | partial | **Done** | confirmed by user; staged download, streamed upload |
| Drop files onto session window | yes | **Done** on X11/XWayland | native Wayland needs winit support; use `--x11` |
| TLS verification, known hosts, changed-cert warning | yes | **Done** | system roots, else TOFU pinning; policies ask / accept-new / strict / insecure |
| X11 and Wayland | both | **Done** | clipboard through XWayland on Wayland |

## 2. Display and performance

| Feature | Feasibility | Status | How |
|---|---|---|---|
| Dynamic resolution on window resize | Medium | **Planned P1** | IronRDP DisplayControl DVC (`--dynamic-resize` exists, experimental); debounce resizes, send `DISPLAYCONTROL_MONITOR_LAYOUT` |
| Scaling toggle (1:1 with scrollbars / fit) | Easy | **Done** | toolbar button; edge-scroll panning, slim scroll indicators (`scale::View`) |
| Instant screenshot to `~/Pictures` | Easy | **Done** | toolbar camera; PNG, XDG pictures dir, never overwrites (`shot.rs`) |
| Send Ctrl+Alt+Del / Win+L | Easy | **Partly done** | Ctrl+Alt+Del: toolbar button + Ctrl+Alt+End. Win+L still to do |
| Pause session / lock local input | Easy | **Done** | toolbar pause: frozen picture, input blocked |
| Text-priority vs media-priority profile | Medium | **Partly done**: speed presets exist; sharpness-vs-fps trade-off is **Planned P2** | RDP perf flags (font smoothing, wallpaper, animations), colour depth 16/32, bitmap codec choice. Real "H.264 media mode" depends on RDPEGFX, see below |
| Colour depth / FPS slider | Medium | **Partly done**: 16-bit via Slow preset; slider and FPS cap **Planned P2** | colour depth is negotiated at connect; FPS cap = client-side present throttle |
| Multi-monitor (span or switchable tabs) | Hard | **Planned P3** | DisplayControl multi-monitor layout + one window per monitor |
| H.264 / H.265 (RDPEGFX) | Hard | **Blocked** | depends on IronRDP graphics-pipeline support; track upstream |
| HiDPI / wgpu renderer | Hard | **Planned P3** | replace softbuffer path, keep software fallback |
| Virtual display driver injection on headless Windows | Hard | **Out of scope** | RDP cannot install drivers. Honest alternative: DisplayControl resolution (P1) plus docs for IddCx virtual display / `Set-DisplayResolution` on the server |
| Single-application streaming (RemoteApp / RAIL) | Hard | **Blocked** | needs RAIL virtual channel, not in IronRDP; server must publish the app |

## 3. Files, clipboard, transfers

| Feature | Feasibility | Status | How |
|---|---|---|---|
| Transfer progress in toolbar (bar, speed, 2 px strip when hidden, done toast) | Medium | **Planned P1** | engine already knows sizes/bytes; add `TransferEvent` to the sink, draw in `ui.rs` |
| Transfers manager (list, pause/resume/cancel, priority, open folder) | Medium | **Planned P2** | queue in `nexdesk-clipboard`; CLIPRDR gives single outstanding paste, so queue is serial by protocol |
| Resume after network drop | Hard | **Planned P3** | CLIPRDR has no resume; only possible for *downloads* via ranged `FileContentsRequest` plus a `.state` file; uploads restart. Custom resume protocol is not possible over stock RDP |
| Collision dialog (overwrite / keep both / skip / newer) | Medium | **Planned P2** | applies to Windows→Linux staging; for Linux→Windows Explorer itself asks |
| Clipboard history (last 5 items) | Medium | **Planned P2** | ring buffer in engine, picker in toolbar |
| Compression (zstd/lz4) | Hard | **Out of scope** | CLIPRDR payload format is fixed by the server; nothing to negotiate. RDP transport compression is separate and already negotiated |
| Drive redirection (RDPDR) | Hard | **Planned P3** | needs IronRDP `rdpdr` backend; Linux folder shared as `\\tsclient\name` |
| Mount remote folder in Nautilus | Hard | **Out of scope** | would need FUSE plus an SMB/RDPDR bridge; use drive redirection instead |
| Drag file *out of* remote onto Linux file manager | Hard | **Out of scope** | not in the RDP protocol; copy + paste works |
| Large-clipboard freeze fix | Easy | **Done** | clipboard engine is its own actor thread, UI never blocks |

## 4. Security and credentials

| Feature | Feasibility | Status | How |
|---|---|---|---|
| Encrypted vault, master password (Argon2id + XChaCha20-Poly1305) | Medium | **Done** (crypto tested; UI unverified) | `nexdesk-core::vault`: 64 MiB / 3-pass Argon2id, random data key wrapped by master password and by recovery key, header + both wraps authenticated, padded body, atomic write, mode 0600, hostile-header limits. Save checkbox in the connect dialog, auto-connect with saved password, Forget password, lock / change master / delete all in Preferences, unlock at start |
| Recovery key | Medium | **Done** (52-character key, shown once, copy button) | the 3-word verification gate and PDF export are **Planned P2** |
| First-launch wizard (vault vs system keyring) | Easy | **Partly done**: vault setup is in Preferences; a first-run wizard and the keyring choice are **Planned P2** | |
| GNOME Keyring / KWallet (Secret Service) | Medium | **Planned P1** | `secret-service` crate; per-app attribute scoping, warn about the "any unlocked-session app can read" limitation |
| Plaintext-never rule | Easy | **Done** | passwords not in profiles or `.rdp` files |
| Auto-lock after idle / on session lock | Easy | **Planned P1** | manual Lock exists; idle timeout and screen-lock hook still to do |
| Zero-trust profile toggle (block clipboard, files, drives) | Easy | **Planned P1** | engine refuses to build the backend; enforced client-side. Admin policy file in P4 |
| TLS only / refuse RDP-security fallback | Easy | **Planned P1** | require TLS or CredSSP, never legacy RDP encryption; setting `Require NLA` |
| Find RDP computers on the local subnet | Medium | **Done** | Devices > Scan network: TCP 3389 on own /24 per interface, reverse-DNS names, Add/Connect buttons (`discover.rs`). mDNS/NetBIOS names and non-default ports are future work |
| Pre-connection baseline check | Medium | **Planned P2** | cert validity/expiry/key size, TLS version, NLA offered; scanning other people's networks is out of scope (legal and unreliable); the Devices scan below only probes your own subnet on request |
| PAM / polkit unlock of vault | Medium | **Planned P3** | polkit action, fingerprint if the system provides it |
| Audit log (connect, disconnect, clipboard/file events, no content) | Medium | **Planned P4** | append-only JSON lines, optional syslog/journald |
| Admin policy file (fleet-wide disable features) | Medium | **Planned P4** | `/etc/nexdesk/policy.toml` overrides profile |
| Kerberos / SSO | Hard | **Planned P4** | IronRDP sspi support |
| Smartcard | Hard | **Blocked** | upstream |

## 5. Workflow and connectivity

| Feature | Feasibility | Status | How |
|---|---|---|---|
| Pre/post-connection command | Easy | **Done** (shell mode not offered) | per-profile; run via argv (no shell) unless the user opts into shell mode; timeout; show output on failure. Treat profile files as untrusted: require confirm when imported |
| SSH tunnel / jump host | Medium | **Planned P2** | spawn `ssh -L` or embed `russh`; connect to the local port; certificate name still checked against the real host |
| Groups, search, quick connect, tabs | Medium | **Planned P2** | folder tree in sidebar, inline connect/edit icons on hover |
| Auto-reconnect | Medium | **Planned P2** | lifecycle states exist; backoff, re-prompt on credential failure |
| Persistent layout / per-workspace window geometry | Easy | **Planned P2** | store in profile |
| Session recording (webm/mp4) | Hard | **Planned P3** | frame tap → `ffmpeg` pipe if installed; opt-in per profile, large privacy warning, never records the password prompt window of the client |
| Audio, printers, USB | Hard | **Planned P3/P4** | IronRDP `rdpsnd` first |
| RD Gateway | Hard | **Blocked** | upstream gateway support |
| NAT hole punching / relay (native protocol) | Infra | **Out of scope** | needs a signaling+relay server and a different (non-RDP-listener) agent on the target. Recommended instead: SSH jump host (P2) or WireGuard/Tailscale; revisit only if you want to operate infrastructure |
| VPN control (bring up a NetworkManager VPN before RDP, wait until reachable, optionally bring down after) | Medium | **Planned P2** | `nmcli connection up/down`, polkit (no sudo), credentials stay in NetworkManager/keyring; never falls back to a direct connection if the VPN fails. Vendor-only clients (AnyConnect etc.) via the pre/post command (P1) |
| VNC and SSH protocols | Hard | **Out of scope for now** | NexDesk is an RDP client; the profile format stays protocol-tagged so it can be added later |

## 6. Packaging and quality

| Item | Status |
|---|---|
| Unit + integration tests (clipboard engine, X11 transport, TLS policy, overlay UI) | **Done** |
| GUI end-to-end test harness (Xvfb + scripted RDP server) | **Planned P2** |
| `.deb` and Flatpak | **Planned P4** (Flatpak needs portal-based file access, review clipboard staging path) |
| Signed releases, SBOM, `cargo audit` in CI | **Planned P4** |

## 7. Native remote control (own agent + protocol + servers)
Architecture and rules: `docs/REMOTE.md`. RDP stays the way to reach Windows Pro/Server; this section covers what RDP cannot.

| Feature | Feasibility | NexDesk | Notes |
|---|---|---|---|
| Control a Windows PC | Easy | **Done (via RDP)** | |
| Control Linux X11 host | Hard | **Done (prototype)** | `nexdesk-agent` + `nexdesk-peer-view`, direct IP, tested on Xvfb; XShm and a video codec still to do (`docs/PEER.md`) |
| Control Linux Wayland host | Hard | **Planned P5b** | xdg-desktop-portal ScreenCast + PipeWire, libei; permission prompt limits unattended use |
| Control macOS host | Hard | **Planned P5d** | ScreenCaptureKit, CGEvent, permission dialogs |
| Connect by ID, no port forwarding | Infra | **Done (prototype, relay only)** | `nexdesk-relay`, `docs/RELAY.md`; hole punching, relay auth and failover still to do |
| Direct IP access | Medium | **Done (prototype)** | agent listens on a port; default 127.0.0.1:21118 |
| Self-hosted ID + relay servers | Infra | **Done (prototype)**, Docker image and hardening planned | `nexdesk-relay` |
| NAT hole punching, relay fallback | Hard | **Planned P5c** | |
| End-to-end encryption, key pinning | Medium | **Done (prototype, unreviewed)** | Noise-style handshake, known_hosts-like pins |
| Unattended access (service, permanent password, allow-list) | Medium | **Planned P5b** | systemd / Windows service |
| Accept/deny prompt, per-session permissions, view-only | Easy | **Done (prototype)** | terminal prompt, `--allow` list, `--view-only`; per-feature toggles later |
| Windows login screen / UAC | Hard | **Planned P5d** | SYSTEM service helper |
| Privacy mode, block remote input | Hard | **Planned P5d** | |
| Audio | Medium | **Planned P5b** | Opus |
| File transfer (queue, resume) | Medium | **Planned P5b** | reuse clipfiles sanitising |
| Clipboard text/images/files | Easy | **Text done (P5a prototype)**, images/files planned | text via `nexdesk-clipboard` X11 transport |
| Chat | Easy | **Planned P5b** | |
| TCP tunnelling | Medium | **Planned P5b** | off by default |
| Multi-monitor, custom resolution | Medium | **Monitor switching done (prototype)**, custom resolution planned | RandR monitor list, one monitor shown at a time |
| Codec / quality / FPS selection, hardware encoders | Hard | **Planned P5a, hardware P5d** | |
| Session recording | Medium | **Planned P5b** | host consent required |
| Wake-on-LAN | Easy | **Planned P2** | no new protocol needed |
| 2FA for incoming connections | Medium | **Planned P5c** | |
| Address book / tags / groups synced between users | Infra | **Planned P5c** | local books exist today |
| Android / iOS / web clients | Hard | **Planned P6** | separate effort |
| Web console, OIDC/LDAP, audit log (Pro) | Infra | **Planned P6** | audit log locally is P4 |
| Plugins | Hard | **Out of scope** | attack surface; revisit later |
| Interoperate with other remote-desktop products | Hard | **Out of scope** | protocol churn and licence issues; we build our own |

### Security design (see `docs/SECURITY_DESIGN.md`)
| Feature | Feasibility | NexDesk | Notes |
|---|---|---|---|
| Hybrid post-quantum key exchange (X25519 + ML-KEM-768) in the NexDesk protocol | Medium | **Done (prototype)** | `nexdesk-crypto`, tested; **needs independent crypto review** and KAT vectors before any real use |
| Dual signatures (Ed25519 + ML-DSA-65) on device identity keys | Medium | **Done (prototype)** | both must verify; fingerprint for pinning; identity store in vault still to do |
| 256-bit symmetric encryption, forward secrecy, rekeying | Easy | **Done (prototype)** | vault already uses XChaCha20-Poly1305 / Argon2id (**Done**) |
| Post-quantum protection for RDP sessions | Medium | **Planned P2** | SSH tunnel with hybrid KEX (OpenSSH 9+/10) or WireGuard PSK; RDP/TLS itself is chosen by the server |
| Quantum key distribution (QKD) | n/a | **Out of scope** | needs dedicated optical hardware |
| Consent by default, per-session permissions (default-deny) | Easy | **Planned P5a** | |
| FIDO2/WebAuthn + TOTP + device certificates (mTLS) | Medium | **Planned P5c** | |
| Signed admin policy enforced in the agent | Medium | **Planned P4/P5c** | |
| Tamper-evident (hash-chained, signed) audit log + SIEM export | Medium | **Planned P4** | local logs exist today |
| Signed releases, SBOM, cargo-audit/deny in CI, reproducible builds | Medium | **Planned P4** | |
| Protocol fuzzing, third-party pentest and crypto review before release | Medium | **Planned P5a-P5c** | release gate |
