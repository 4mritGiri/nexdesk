# Vendored IronRDP crates

Both are unmodified copies of the crates.io releases except for the marked `NexDesk patch` blocks,
wired in through `[patch.crates-io]` in the workspace `Cargo.toml`.

| crate | version | patch |
|---|---|---|
| `ironrdp-client` | 0.1.0 | `RdpClient::set_clipboard_factory` (stock code hard-wires a no-op stub clipboard on Linux) and `Config::set_tls_verifier` |
| `ironrdp-tls`    | 0.2.2 | `upgrade_with_verifier` (stock `upgrade` accepts every certificate **and** skips handshake signature checks) |

When upgrading IronRDP, re-apply these small diffs (`grep -rn "NexDesk patch" vendor`) or upstream them.
