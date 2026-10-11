# Changelog

All notable changes are listed here, newest first. Format: [Keep a Changelog](https://keepachangelog.com/), versions follow [SemVer](https://semver.org/).

## [Unreleased]
### Added
- Vector (SVG path) icons in the native viewer header and chat; chat has a send button. Remote Control page of the manager redesigned: cards with icons, status pill, ID box, chat bubbles, new SVG icons.
- Native viewer window now looks and behaves like the RDP viewer: own header with traffic-light buttons, connection tab and icon tools (Ctrl+Alt+Del, copy screen, 1:1, pause/resume control, chat), drag to move, double-click to maximise, resize from the edges, floating pin-able bar in full screen. `--native-frame` keeps the system title bar.
### Fixed
- Native viewer on your own computer: control is now refused (and starts off) because the remote pointer is your real pointer, which made the mouse unable to leave the window and clicks, chat and shortcuts miss. Detected by the agent's fingerprint, so an ID through a relay is recognised too. `--allow-local-control` overrides for testing.
- Native viewer: the full-screen toolbar could not be reached (a 2 px gap between the reveal zone and the bar closed it again); chat typing now uses the text the keyboard produces (works with any layout or input method).
- Relay access key: `nexdesk-relay --key-file` (or `NEXDESK_RELAY_KEY`) limits the relay to agents and viewers that know the key (HMAC challenge-response, replay-safe). Dockerfile for self-hosting: `packaging/docker/Dockerfile.relay`.
- Native remote control: agent and viewer, connect by ID through a relay, file transfer, chat, Wayland sharing (portal + PipeWire).
- Viewer overlay redesign, control toggle (Ctrl+Alt+G), chat panel.
- Platform backends layout (`platform/`), portable clipboard, key map, per-OS paths.
- Repository restructure: `apps/`, `crates/`, `docs/{product,architecture,security,adr}`, fuzz targets, CI.
