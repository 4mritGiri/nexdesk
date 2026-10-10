# NexDesk vision

**NexDesk is one open-source program for reaching any computer: remote desktop, remote support, a multi-protocol
connection manager and a terminal, on Linux, Windows and macOS, that you can run entirely on your own servers.**

It is meant to replace, with a single free and auditable tool:

| Job | What NexDesk offers (target) |
|---|---|
| Remote support and unattended access to a computer by ID, through NAT | The native stack: agent, viewer, rendezvous and relay server, consent prompts, unattended mode with a strong secret |
| Windows Remote Desktop (RDP) client | The RDP engine today: saved connections, tabs, clipboard, files, certificate pinning, multi-monitor next |
| A manager for many kinds of connections | Address books, tags, search, import/export, one vault for secrets, RDP now; SSH, VNC and the native protocol in the same window |
| A modern terminal with SSH | Local shell and SSH in tabs and split panes, key and agent support, port forwarding, an SFTP side panel, themes, saved hosts shared with the connection manager |
| Self-hosted infrastructure | A small relay/rendezvous server (single binary, Docker image), no account, no cloud, no telemetry |

## Principles

1. **Open source, permissive licence.** `MIT OR Apache-2.0` (see `LICENSE-MIT`, `LICENSE-APACHE`). Anyone can use, audit, fork and self-host it.
2. **Own code.** NexDesk is written from scratch on permissively licensed libraries. We do not copy source from other
   remote-desktop projects, whatever their licence; contributions must be original or compatible-licensed and credited.
3. **Secure by default.** Consent on every incoming connection, pinned keys, encrypted transport end to end (the relay sees only
   ciphertext), secrets never in plain files or command lines, a hybrid post-quantum handshake (`docs/security/SECURITY_DESIGN.md`).
   Unattended access is opt-in and says so loudly. Security reviews and fuzzing come before new network features.
4. **No lock-in, no tracking.** Plain-file formats where possible (`.rdp`, OpenSSH config import), no telemetry, no account.
5. **Native on Linux first, honest about status.** X11 and Wayland both work; every feature in the matrix says
   whether it is done, unverified or planned. Nothing is called finished until it has run on a real machine.

## Product pillars and where they stand

| Pillar | State | Plan |
|---|---|---|
| RDP client | Working, in daily use | P1–P3 in `docs/product/ROADMAP.md`: resize, multi-monitor, drive redirection, audio, gateway |
| Native remote control (agent, viewer, relay) | Prototype: X11 and Wayland host, files, chat, connect by ID via relay | P5: hole punching, unattended mode, Windows and macOS hosts, audio, recording, review of the crypto |
| Connection manager | Working for RDP | P2: tags, groups, sync file, SSH/VNC entries, shared address books (server tier) |
| Terminal and SSH | Not started | **P7** below |
| Server | Relay with ID registration | P5c: hole punching, relay auth, Docker image, optional web console |
| Platforms | Linux | Windows and macOS clients first, then hosts; mobile and web later |

## P7 – Terminal and SSH (new crate `nexdesk-term`)

Built on a terminal emulation library and a pure-Rust SSH client library (both permissive); GPUI renders the grid.
1. Local shell tabs: PTY, resize, scrollback, copy/paste, search, font and theme settings.
2. SSH: password, key file, agent, jump host, host-key pinning with the same known-hosts policy as RDP, keepalive, reconnect.
3. Split panes, tab groups, broadcast input, snippets, session logging to file (opt-in).
4. Port forwarding (local, remote, dynamic), shown in the connection manager; the same tunnel can carry RDP.
5. SFTP panel: browse, upload, download, drag and drop, resume, queue shared with RDP file transfers.
6. Saved hosts shared with the connection manager and vault; import from OpenSSH config.
7. Serial and telnet only if asked for.
Exit criteria: day-to-day shell and SSH use for a week by the maintainers, terminal conformance tests (vttest-style) in CI.

## Release path to "usable replacement"

1. **0.3** native remote control usable between two Linux desktops by ID, with documented self-hosting, Windows and macOS builds of the RDP client.
2. **0.4** terminal and SSH, connection manager unified across protocols.
3. **0.5** hole punching, unattended access, Windows host agent, crypto review published.
4. **1.0** signed packages for Linux, Windows and macOS, security policy and audit report, stable file formats.

## Community

Contributing guide, security policy (`SECURITY.md`) and a code of conduct accompany the first public release.
Good first tasks are tagged in the issue tracker; anything touching `nexdesk-crypto` needs two reviewers.
