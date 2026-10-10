# 4. `apps/` and `crates/`
Status: accepted

**Decision.** Binary-only programs live in `apps/` (`manager`, `rdp-viewer`); libraries live in `crates/`. `nexdesk-peer` and `nexdesk-network` keep thin `src/bin/` wrappers so their integration tests can use `CARGO_BIN_EXE_*`; all logic is in their libraries.
**Consequences.** Dependencies point one way (apps -> crates). A program's `main` only calls a library function.
