# Architecture

```
crates/
  nexdesk-crypto      identities, hybrid handshake, encrypted records (no I/O policy)
  nexdesk-network     relay/rendezvous protocol and server (`nexdesk-relay`), ID registration
  nexdesk-core        profiles, .rdp files, vault, known hosts, logs, scaling: no UI, no OS-specific code except small cfg(unix) bits
  nexdesk-clipboard   RDP clipboard backend (X11 selection protocol)
  nexdesk-session     RDP session lifecycle in the manager
  nexdesk-renderer    framebuffer abstraction
  nexdesk-rdp         RDP session window (IronRDP, winit, softbuffer)
  nexdesk-ui          manager UI (GPUI)
  nexdesk             manager executable
  nexdesk-peer        native remote control
    src/wire.rs         message format          src/link.rs     encrypted framing
    src/host.rs         agent session loop      src/agent.rs    agent program
    src/viewer/         viewer program          src/overlay/    viewer overlay drawing (toolbar, chat, font)
    src/platform/       per-OS capture + input  src/frame.rs    picture differencing
    src/xfer.rs         file transfer           src/clip/       clipboard sync
    src/keymap.rs paths.rs store.rs local.rs shot.rs control.rs   small, single-purpose helpers
    src/bin/            thin `main` wrappers only
vendor/               patched IronRDP crates (see vendor/README.md)
docs/                 design, protocol and status documents
packaging/            .deb, desktop files, install script
```

Rules: lower crates never depend on higher ones; OS-specific code lives only in `platform/` (peer), `grab.rs` (rdp) and small `cfg` blocks; a program's `main` only calls a library function so everything is testable; security-sensitive crates (`nexdesk-crypto`, relay, consent logic in `host.rs`/`agent.rs`) need two reviewers (`CONTRIBUTING.md`).
