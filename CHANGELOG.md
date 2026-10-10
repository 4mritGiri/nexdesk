# Changelog

All notable changes are listed here, newest first. Format: [Keep a Changelog](https://keepachangelog.com/), versions follow [SemVer](https://semver.org/).

## [Unreleased]
### Added
- Relay access key: `nexdesk-relay --key-file` (or `NEXDESK_RELAY_KEY`) limits the relay to agents and viewers that know the key (HMAC challenge-response, replay-safe). Dockerfile for self-hosting: `packaging/docker/Dockerfile.relay`.
- Native remote control: agent and viewer, connect by ID through a relay, file transfer, chat, Wayland sharing (portal + PipeWire).
- Viewer overlay redesign, control toggle (Ctrl+Alt+G), chat panel.
- Platform backends layout (`platform/`), portable clipboard, key map, per-OS paths.
- Repository restructure: `apps/`, `crates/`, `docs/{product,architecture,security,adr}`, fuzz targets, CI.
