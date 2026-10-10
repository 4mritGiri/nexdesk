# Architecture

```
apps/                 programs that only wire libraries together (binary-only crates)
  manager/              `nexdesk`: manager window (GPUI)
  rdp-viewer/           `nexdesk-rdp`: RDP session window (IronRDP, winit, softbuffer)
crates/               libraries
  nexdesk-crypto        identities, hybrid handshake, encrypted records (no I/O policy)
  nexdesk-network       relay/rendezvous protocol and server (`nexdesk-relay` wrapper in src/bin)
  nexdesk-core          profiles, .rdp files, vault, known hosts, logs, scaling: no UI, little OS-specific code
  nexdesk-clipboard     RDP clipboard backend (X11 selection protocol)
  nexdesk-session       RDP session lifecycle in the manager
  nexdesk-renderer      framebuffer abstraction
  nexdesk-ui            manager UI (GPUI)
  nexdesk-peer          native remote control (agent + viewer libraries; thin wrappers in src/bin)
    src/wire.rs         message format          src/link.rs     encrypted framing
    src/host.rs         agent session loop      src/agent.rs    agent program
    src/viewer/         viewer program          src/overlay/    viewer overlay drawing (toolbar, chat, font)
    src/platform/       per-OS capture + input  src/frame.rs    picture differencing
    src/xfer.rs         file transfer           src/clip/       clipboard sync
    src/keymap.rs paths.rs store.rs local.rs shot.rs control.rs   small, single-purpose helpers
vendor/               patched IronRDP crates (see vendor/README.md)
fuzz/                 cargo-fuzz targets for every parser of network input
docs/                 product/ architecture/ security/ adr/ (see docs/README.md)
packaging/            .deb, desktop files, install script
scripts/              maintenance helpers (clean-stale.sh, rename-brand.sh)
.github/              CI, release, CODEOWNERS, templates, dependabot
```

Rules: lower crates never depend on higher ones; OS-specific code lives only in `platform/` (peer), `grab.rs` (rdp) and small `cfg` blocks; a program's `main` only calls a library function so everything is testable; security-sensitive crates (`nexdesk-crypto`, relay, consent logic in `host.rs`/`agent.rs`) need two reviewers (`CONTRIBUTING.md`).
