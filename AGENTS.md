 ## 1\. Purpose

 This repository is NexDesk, a Rust workspace for a remote desktop application.

 These instructions apply to all AI coding agents and contributors.

 Priority order:

 1. Correctness
2. Security
3. Preserve existing behavior
4. Maintainability
5. Performance
6. Minimal, reviewable diffs

 Do not sacrifice correctness, security, or compatibility for theoretical optimization.

---

 ## 2\. Repository Structure

```
crates/
├── nexdesk/              # Application entry point
├── nexdesk-core/         # Core domain logic and persistent state
├── nexdesk-clipboard/    # Clipboard synchronization and file transfer
├── nexdesk-rdp/          # RDP client, viewer, input, capture, TLS, diagnostics
├── nexdesk-renderer/     # Rendering
├── nexdesk-session/      # Session lifecycle and coordination
└── nexdesk-ui/           # UI state, navigation, theme, assets

vendor/
├── ironrdp-client/       # Vendored RDP client
└── ironrdp-tls/          # Vendored TLS support

docs/                     # Architecture and product documentation
packaging/                # Installers and packaging scripts
```

 ### Crate responsibilities

 - `nexdesk`
  - Application startup and top-level orchestration.
  - Keep `main.rs` thin.
- `nexdesk-core`
  - Domain logic.
  - Profiles.
  - Credentials and vault.
  - Settings.
  - Known hosts.
  - Discovery.
  - Persistent application state.
- `nexdesk-clipboard`
  - Clipboard conversion.
  - Clipboard transport.
  - Clipboard synchronization.
  - Clipboard file transfer.
  - Platform-specific clipboard support.
- `nexdesk-rdp`
  - RDP connections.
  - RDP protocol integration.
  - TLS.
  - Input and key mapping.
  - Screen capture.
  - Viewer runtime.
  - RDP diagnostics.
- `nexdesk-renderer`
  - Rendering abstractions and implementation.
  - Do not put application business logic here.
- `nexdesk-session`
  - Session lifecycle.
  - Session state.
  - Coordination between core, RDP, clipboard, and UI.
- `nexdesk-ui`
  - UI state.
  - Navigation.
  - Theme.
  - Assets.
  - UI-specific behavior.

 Put new code in the smallest crate that owns the responsibility.

 Do not use `nexdesk-core` as a generic dumping ground.

---

## NexDesk Architecture

NexDesk is an enterprise remote-access platform, not only an RDP client.

Supported or planned connection types include:

- RDP
- SSH
- NexDesk native remote sessions
- Secure file transfer
- Port forwarding
- Network/VPN access

Architecture must keep connection protocols independent from the
application UI and domain state.

### Security boundaries

- `nexdesk-core` owns application/domain state.
- `nexdesk-session` owns session lifecycle and protocol orchestration.
- `nexdesk-crypto` owns cryptographic primitives and cryptographic
  protocol building blocks.
- `nexdesk-rdp` owns RDP-specific behavior.
- `nexdesk-ssh` owns SSH-specific behavior when introduced.
- `nexdesk-network` owns relays, rendezvous, proxies, and network
  transport when introduced.
- `nexdesk-vpn` owns network-tunnel functionality when introduced.
- `nexdesk-ui` must not implement security decisions.
- Security policy must be enforced below the UI layer.

Never trust a UI-only permission check for security.

A capability denied by policy must remain denied even if a client
modifies or bypasses the UI.

### Security principle

Security-critical decisions must be enforced at the lowest layer
that owns the protected resource.

The UI may display permissions and request operations, but it must
never be the authority that grants security-sensitive capabilities.

 ## 3\. Architecture

 ### Crate boundaries

 Before adding a dependency between crates, determine:

 1. Which crate owns the concept?
2. Whether the dependency creates an architectural cycle.
3. Whether the feature can remain inside the existing crate.
4. Whether a small interface is preferable to exposing implementation details.

 Prefer narrow dependencies.

 Avoid unnecessary cross-crate coupling.

 ### Executable code

 Keep `crates/nexdesk/src/main.rs` thin.

 Business logic belongs in library crates.

 ### Abstractions

 Do not introduce a trait, manager, registry, coordinator, event bus, service layer, or generic framework unless the requested feature actually requires it.

 Prefer concrete code over abstractions used by only one caller.

 ### Feature-specific state

 Keep feature-specific state in the feature's own module or crate.

 Do not add feature-specific fields to broad shared structs unless necessary.

---

 ## 4\. Minimal-Diff Policy

 Make the smallest correct change that solves the requested problem.

 Do not:

 - reformat unrelated files;
- rename unrelated code;
- refactor unrelated code;
- modernize unrelated APIs;
- fix unrelated bugs;
- reorganize modules without necessity;
- replace working implementations without a concrete reason.

 A small amount of duplication is preferable to a broad refactor when it reduces regression risk.

 ### Preserve existing paths

 When adding a feature, preserve existing behavior when the feature is:

 - disabled;
- unsupported;
- unavailable on the current platform;
- unavailable due to configuration.

 Prefer adding a new path over rewriting an existing path.

 ### Final minimization pass

 Before considering a change complete:

 1. Inspect every modified file.
2. Inspect every modified existing code path.
3. Remove unrelated changes.
4. Verify existing behavior remains unchanged where possible.
5. Check whether shared code was modified unnecessarily.
6. Check whether new state can remain local.
7. Check whether dependencies can be avoided.

 If a change cannot be justified by the requested behavior, remove it.

---

 ## 5\. Rust Style

 Use idiomatic Rust and follow the Rust API Guidelines.

 Required:

 - `snake_case` for functions, variables, and modules;
- `PascalCase` for types and traits;
- `SCREAMING_SNAKE_CASE` for constants;
- 4 spaces for indentation;
- `rustfmt`;
- meaningful names;
- exhaustive pattern matching where practical;
- early returns to reduce nesting;
- `enumerate()` instead of manual counters;
- iterators when they improve clarity;
- explicit `.clone()` for non-`Copy` values.

 Avoid:

 - wildcard imports except appropriate preludes/tests;
- unnecessary macros;
- unnecessary generics;
- unnecessary traits;
- hidden allocations;
- clever code that obscures ownership or control flow.

 Do not use emoji or decorative Unicode in source code, comments, or logs.

---

 ## 6\. Imports

 Use one import block per crate.

 Prefer:

```
use base::{fs, message_proto::*};
```

 over:

```
use base::fs;
use base::message_proto::*;
```

 Split imports only when required by:

 - `#[cfg(...)]`;
- visibility/re-export requirements;
- Rust syntax.

 Organize imports as:

 1. Standard library
2. External crates
3. Local crates/modules

 Use `rustfmt`.

---

 ## 7\. Error Handling

 Production code must not use `.unwrap()`.

 Avoid `.expect()` except for genuine programmer invariants.

 Good:

```
let value = operation()?;
```

 Acceptable:

```
let value = invariant
    .expect("session state must exist after successful initialization");
```

 Tests may use `unwrap()` where appropriate.

 Use:

 - `thiserror` for library/domain errors;
- `anyhow` for application-level error aggregation.

 Propagate errors with `?`.

 Do not silently ignore errors.

 When using `anyhow`, provide useful context:

```
operation()
    .context("failed to establish RDP connection")?;
```

---

 ## 8\. Functions

 Functions should have one clear responsibility.

 Prefer:

 - borrowing over unnecessary ownership;
- fewer parameters;
- configuration structs when more than five parameters are required;
- early returns;
- clear control flow.

 Avoid deeply nested logic.

 Do not create abstractions merely to reduce a few duplicated lines.

---

 ## 9\. Structs and Enums

 Keep types focused.

 Use private fields by default.

 Derive appropriate traits such as:

```
Debug
Clone
PartialEq
Eq
Default
```

 when they are actually useful.

 Prefer `Option<T>` over sentinel values.

 Use newtypes when they prevent mixing semantically different values.

 Prefer composition over inheritance-like designs.

---

 ## 10\. Async and Tokio

 Assume a Tokio runtime already exists.

 Never:

 - create nested runtimes;
- call `Runtime::block_on()` inside async code;
- hide runtime creation inside libraries;
- use `std::thread::sleep()` inside async code;
- hold locks across `.await`.

 Use:

```
tokio::spawn(...)
```

 for independent async work.

 Use:

```
tokio::task::spawn_blocking(...)
```

 for blocking or CPU-heavy work.

 Prefer channels for asynchronous coordination.

---

 ## 11\. Concurrency

 Use:

 - Tokio for asynchronous I/O;
- Rayon for CPU-bound parallelism;
- channels for message passing.

 Do not add parallelism without a workload that benefits from it.

 Avoid `Mutex` when `RwLock` or a lock-free design is clearly more appropriate.

 Do not introduce complex concurrency mechanisms for small workloads.

---

 ## 12\. Performance

 Optimize actual bottlenecks, not hypothetical ones.

 Priority:

 1. Correct algorithm and data structure
2. Avoid unnecessary work
3. Avoid unnecessary allocations and copies
4. Efficient I/O
5. Appropriate concurrency
6. Low-level optimization only when justified

 Do not automatically add:

 - SIMD;
- Rayon;
- lock-free structures;
- caches;
- custom allocators;
- unsafe code.

 Use the simplest implementation that satisfies the actual requirements.

 Do not claim performance improvements without measurement.

---

 ## 13\. Memory and Allocation

 Avoid unnecessary:

 - `.clone()`;
- `String` allocations;
- temporary `Vec`s;
- serialization/deserialization;
- frame copies;
- clipboard copies.

 Prefer `&str` and slices when ownership is unnecessary.

 Use `Vec::with_capacity()` when the size is known or cheaply estimated.

 Use `Cow` when conditional ownership is actually useful.

 Do not contort APIs solely to eliminate harmless allocations.

---

 ## 14\. Networking and RDP

 Treat all remote data as untrusted.

 Validate:

 - lengths;
- counts;
- offsets;
- indexes;
- message types;
- encoded strings;
- allocation sizes.

 Never allocate unbounded memory based directly on peer-controlled values.

 Network operations should:

 - handle disconnects;
- propagate errors;
- avoid blocking async workers;
- avoid unbounded buffering;
- use appropriate timeouts.

 Do not assume a remote peer is trustworthy.

---

 ## 15\. TLS and Security

 Never weaken TLS verification merely to make a connection work.

 Do not:

 - disable certificate verification as a generic workaround;
- silently accept invalid certificates;
- log private keys;
- log passwords;
- log authentication tokens.

 If certificate verification behavior changes, add an appropriate regression/security test.

 Security-sensitive behavior should prefer explicit failure over silent fallback.

---

 ## 16\. Credentials and Secrets

 Never commit:

 - passwords;
- API keys;
- authentication tokens;
- private keys;
- credentials;
- `.env` files containing secrets.

 Never log secrets or credential-bearing URLs.

 Use secure storage where appropriate.

 Use `secrecy` for sensitive values when it provides meaningful protection.

 Keep secrets' lifetime and number of copies as small as practical.

 Check `Debug` implementations for credential-bearing structs.

 Ensure `.env` is ignored by Git when applicable.

---

 ## 17\. Clipboard Security

 Clipboard data is untrusted external input.

 Do not assume clipboard content is:

 - small;
- UTF-8;
- text;
- safe;
- local-only.

 Validate sizes and formats before processing.

 Be careful with:

 - file lists;
- paths;
- URIs;
- serialized formats;
- remote clipboard messages.

 Never log clipboard contents.

---

 ## 18\. Logging

 Use the project's logging system.

 Prefer:

```
log::error!(...)
```

 or the existing tracing/logging equivalent.

 Do not use `println!()` for operational error reporting.

 Never log:

 - passwords;
- tokens;
- private keys;
- clipboard contents;
- sensitive personal information.

 High-frequency operations such as:

 - packets;
- frames;
- input events;
- clipboard events;
- polling loops;

 must not emit unthrottled `debug` or higher logs.

 Use `trace` for expected high-frequency diagnostics.

 Use the project's throttled logging mechanism for repeated faults that need to remain visible.

---

 ## 19\. Unsafe Code

 Avoid `unsafe`.

 Use `unsafe` only when safe Rust cannot reasonably provide the required behavior.

 Every `unsafe` block must document its safety invariant.

 Keep unsafe code isolated behind safe interfaces whenever possible.

---

 ## 20\. Dependencies

 Do not add dependencies unless necessary.

 Before adding a crate:

 1. Check the standard library.
2. Check existing workspace dependencies.
3. Check whether another existing dependency already provides the functionality.
4. Consider compile-time and binary-size impact.
5. Consider maintenance and licensing.
6. Consider transitive dependencies.

 Do not add a crate solely to save a few lines of simple code.

 Do not upgrade unrelated dependencies.

 Do not manually edit `Cargo.lock`.

 Use Cargo for dependency management.

---

 ## 21\. Vendor

 `vendor/` contains third-party code.

 Do not modify vendored code unless the requested change specifically requires it.

 Before modifying `vendor/`:

 1. Verify the behavior cannot be changed in NexDesk.
2. Identify the exact upstream component.
3. Keep the modification minimal.
4. Avoid unrelated formatting/refactoring.
5. Explain why the vendor change is necessary.

 Do not perform unrelated vendor upgrades.

---

 ## 22\. Documentation

 Update documentation when a change modifies:

 - public behavior;
- architecture;
- security assumptions;
- supported functionality;
- installation;
- packaging;
- configuration;
- protocol behavior.

 Relevant documentation includes:

```
README.md
docs/architecture/ARCHITECTURE.md
docs/security/SECURITY_DESIGN.md
docs/architecture/CLIPBOARD.md
docs/architecture/VIEWER.md
docs/product/FEATURE_MATRIX.md
docs/product/ROADMAP.md
docs/architecture/REMOTE.md
```

 Do not modify documentation merely to restate obvious implementation details.

---

 ## 23\. Public APIs

 Public functions, structs, enums, traits, and methods should have useful Rustdoc.

 Document important:

 - behavior;
- invariants;
- parameters;
- return values;
- errors;
- safety requirements.

 Add examples for complex public APIs.

 Do not write tautological documentation.

 Bad:

```
/// Gets the name.
pub fn name(&self) -> &str
```

 Prefer documentation that explains non-obvious semantics.

---

 ## 24\. Testing

 Every behavior change should have an appropriate regression test when practical.

 For bug fixes:

 1. Reproduce the reported behavior.
2. Add a focused regression test.
3. Verify the test fails before the fix when practical.
4. Verify it passes after the fix.

 Normally one to three focused tests are preferable to a large test suite addition.

 Test observable behavior rather than implementation details.

 Use:

```
#[cfg(test)]
mod tests {
    ...
}
```

 Run:

```
cargo test
```

 Do not add large test infrastructure for small fixes unless necessary.

---

 ## 25\. Platform-Specific Code

 Keep platform-specific behavior isolated.

 Use narrow `#[cfg(...)]` blocks.

 Do not spread platform-specific conditions throughout shared business logic when the behavior can be isolated.

 Keep unsupported platforms compiling whenever practical.

---

 ## 26\. UI

 `nexdesk-ui` owns:

 - UI state;
- navigation;
- theme;
- assets;
- UI-specific behavior.

 Do not put the following into UI modules merely because the UI invokes them:

 - credential storage;
- RDP protocol logic;
- network protocol handling;
- clipboard transport;
- persistence.

 UI code should call narrow application/session APIs.

 Avoid unnecessary cross-component mutable state.

---

 ## 27\. Assets and Packaging

 Only modify:

```
crates/nexdesk-ui/assets/
packaging/
```

 when required.

 For new assets:

 - use descriptive names;
- use an appropriate format;
- avoid duplicates;
- update required references.

 For packaging changes:

 - preserve existing installation behavior;
- avoid machine-specific paths;
- test scripts when practical;
- do not silently change installation locations.

---

 ## 28\. Localization

 If localization files exist for the feature:

 - change only required entries;
- preserve keys;
- preserve placeholders;
- preserve escape sequences;
- do not translate product names or technical identifiers;
- do not change existing translations unnecessarily.

 Follow the repository's existing localization architecture.

 Do not invent a new localization system for a feature that does not use one.

---

 ## 29\. Git

 Do not:

 - commit secrets;
- commit debug statements;
- commit commented-out code;
- commit temporary files;
- modify unrelated files.

 Do not create commits unless explicitly requested.

 Do not:

 - reset user changes;
- overwrite unrelated work;
- rebase;
- squash;
- rewrite history;

 unless explicitly requested.

 Preserve the user's existing working-tree changes.

---

 ## 30\. Submodules

 Before changing a submodule:

 1. Determine whether the change can remain in NexDesk.
2. Inspect the exact revision.
3. Avoid unrelated updates.
4. Document why the submodule change is required.

 Do not update a submodule merely because a newer version exists.

---

 ## 31\. Review Rules

 When reviewing a PR:

 - review only the introduced diff;
- verify ownership of changed lines;
- do not report pre-existing problems as new findings;
- do not report unrelated cleanup as required work.

 Each finding should explain:

 1. What changed.
2. What breaks.
3. The concrete consequence.
4. Why the problem is introduced by this diff.
5. The smallest reasonable fix.

 Severity must be based on actual consequence.

 Do not inflate severity because a reviewer label says `P1`, `Critical`, or `Major`.

---

 ## 32\. Corner Cases

 Do not automatically introduce:

 - caches;
- timers;
- counters;
- eviction rules;
- expiry logic;
- cross-component references;
- lifecycle state;
- complex coordination;

 to handle rare corner cases.

 First determine whether:

 1. The current code already fails safely.
2. The case is user-reported.
3. A small local change solves it.
4. Fixing it requires new state.
5. It causes crashes, data loss, corruption, or security problems.

 Prefer clean failure over complex machinery.

 If a rare case requires broad architectural changes and does not affect correctness, security, data integrity, or crashes, document it as a known limitation instead.

---

 ## 33\. Regression-Surface Check

 Before handoff, perform a final regression review.

 For every modified existing file:

 - identify the existing behavior affected;
- explain why the modification is necessary;
- verify the old path remains intact where possible;
- verify appropriate tests exist.

 Prefer:

```
new feature
    |
    v
small existing hook
    |
    v
existing implementation remains unchanged
```

 Avoid:

```
new feature
    |
    v
rewrite shared abstraction
    |
    +--> modify unrelated caller
    +--> change legacy behavior
    +--> add compatibility state
    +--> add unnecessary tests
```

 If repeated review fixes cause the implementation to grow significantly, stop and reconsider the design instead of adding more state.

---

 ## 34\. Formatting, Build, Test, and Lint

 Use the repository toolchain from:

```
rust-toolchain.toml
```

 Before handoff, run the applicable checks:

```
cargo fmt --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

 Do not claim a command passed unless it was actually run.

 If a check cannot be run because of environment limitations, report that explicitly.

---

 ## 35\. Benchmarking

 When benchmarks exist:

 - use release mode;
- never run benchmarks in parallel;
- never use `target-cpu=native`;
- never manipulate benchmarks;
- use equivalent workloads;
- disable caches when comparing cold behavior;
- print results to the console.

 Do not save benchmark results unless explicitly requested.

 If performance work is requested, report every benchmark actually run.

 Example:

 | Benchmark | Before | After | Change |
| --- | --- | --- | --- |
| operation | 10 ms | 7 ms | -30% |

Never invent benchmark results.

 If no baseline exists, state that no baseline was available.

---

 ## 36\. Definition of Done

 A change is complete when applicable requirements are satisfied.

 ### Required

 - [ ] Requested behavior is implemented.
- [ ] Existing behavior is preserved where not intentionally changed.
- [ ] No unrelated cleanup is included.
- [ ] No unnecessary dependencies were added.
- [ ] No secrets were introduced.
- [ ] Errors are handled correctly.
- [ ] Relevant regression tests exist.
- [ ] `cargo fmt --check` passes.
- [ ] `cargo check --workspace` passes.
- [ ] `cargo test --workspace` passes.
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` passes when applicable.

 ### When applicable

 - [ ] Public API documentation updated.
- [ ] Architecture documentation updated.
- [ ] Security documentation updated.
- [ ] Packaging updated.
- [ ] Platform-specific behavior tested.
- [ ] Performance benchmark run.
- [ ] Benchmark results reported.
- [ ] Vendor changes documented.
- [ ] Submodule changes inspected.
- [ ] UI assets verified.
- [ ] Localization updated.

---

 ## 37\. Agent Workflow

 For non-trivial tasks:

 1. Inspect the relevant crate and nearby code.
2. Identify the smallest implementation surface.
3. Check existing APIs before creating new ones.
4. Implement the smallest correct change.
5. Add focused regression tests.
6. Run formatting, build, tests, and linting.
7. Inspect the final diff.
8. Remove unrelated changes.
9. Review the regression surface.
10. Report exactly what changed and what was verified.

 Do not ask for clarification when the repository and request provide enough information to make a reasonable implementation.

 Ask only when proceeding would require guessing an important:

 - functional requirement;
- security requirement;
- compatibility requirement;
- product requirement.

---

 ## 38\. Final Response

 When reporting completed work, provide:

 ### Summary

 What was implemented.

 ### Files Changed

 Only files actually changed and why.

 ### Tests

 Commands actually run and their results.

 ### Regression Surface

 Existing paths that changed and why.

 If no existing behavior changed outside the requested feature, state that explicitly.

 ### Known Limitations

 Only real limitations discovered during implementation.

 ### Performance

 If performance was relevant:

 - what was measured;
- actual results;
- benchmark configuration;
- whether improvement was demonstrated.

 Never claim verification that was not performed.

---

 ## 39\. Core Principle

 > Make the smallest correct, secure, maintainable change that fits the existing architecture.

 Do not optimize code merely for theoretical performance.

 Do not add abstractions merely because they are reusable.

 Do not modify existing behavior unless the requested change requires it.

 Do not increase the regression surface without a clear reason.

 Correctness and security come first.

 Performance should be measured when it matters.
