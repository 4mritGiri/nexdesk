# 3. Per-OS backends behind one API
Status: accepted

**Decision.** Everything that depends on the operating system (screen capture, input injection, key numbering, clipboard, directories) sits in `crates/nexdesk-peer/src/platform/` and a few small modules; backends expose the same API, selected with `cfg(target_os)`. The session loop is platform independent.
**Consequences.** A new OS needs one new backend and no change to protocol or session code. Systems without a backend compile and report a clear message.
