# NexDesk

NexDesk is an enterprise-oriented Rust RDP client built on IronRDP with a GPUI desktop manager and an isolated session/process boundary.

## Workspace

```text
crates/
├── nexdesk-core/       # RDP files, profiles, credentials boundary, scaling, secure file staging
├── nexdesk-session/    # typed lifecycle, SessionManager, reconnect foundation, metrics
├── nexdesk-renderer/   # framebuffer/display abstraction
├── nexdesk-ui/         # GPUI manager UI, navigation and theme
├── nexdesk-rdp/        # existing IronRDP engine/input/render loop, migrated from myrdp-viewer
└── nexdesk/            # GPUI desktop executable
```

### Security boundaries

- Saved `.rdp` profiles never persist passwords.
- Passwords are represented by an opaque `Secret` and are not `Debug` printable.
- The manager passes a password to the RDP child through `NEXDESK_PASSWORD`, never a command-line argument.
- Remote clipboard file names are sanitized before staging.
- RDP protocol execution is isolated in `nexdesk-rdp`; the manager owns lifecycle state.
- TLS verification still follows the underlying IronRDP 0.1.0 behavior and must be hardened before enterprise production.

## Build

This source package was migrated against the current GPUI 0.2.2 / gpui_platform 0.1.0 API shape and retains the project's exact IronRDP 0.1.0 pin.

```bash
cargo test -p nexdesk-core
cargo test -p nexdesk-session
cargo check --workspace
cargo build --release --workspace
```

The build should be run on a current stable Rust toolchain with the Linux development packages required by GPUI.

## Running

Build both `nexdesk` and `nexdesk-rdp` so the manager can find the engine beside its executable:

```bash
cargo build --release -p nexdesk -p nexdesk-rdp
./target/release/nexdesk
```

For direct protocol testing:

```bash
NEXDESK_PASSWORD='...' ./target/release/nexdesk-rdp --host jump.example.com -u alice
```

## Migration status

The original `myrdp` core/viewer/app crates were converted into the NexDesk crate names rather than discarded. The existing IronRDP connection, keyboard, mouse, scaling and clipboard behavior remains in `nexdesk-rdp`/`nexdesk-core`.

The new session layer is deliberately a foundation: reconnect policy and OS keyring storage are exposed as boundaries so they can be added without coupling the GPUI UI to IronRDP internals.
