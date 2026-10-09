# Vendored IronRDP crates

These are unmodified copies of the crates.io releases except for the marked `NexDesk patch` blocks,
wired in through `[patch.crates-io]` in the workspace `Cargo.toml`.

| crate | version | patch |
|---|---|---|
| `ironrdp-client` | 0.1.0 | `RdpClient::set_clipboard_factory` (stock code hard-wires a no-op stub clipboard on Linux), `Config::set_tls_verifier`, a 15 s limit on the TCP connect (stock code waits for the OS timeout, minutes), time limits on negotiation (20 s), TLS (180 s, includes the certificate question) and sign-in (60 s), and `RdpOutputEvent::Stage` so the window can say which step is running |
| `ironrdp-connector` | 0.10.0 | accept a server that skips licensing and sends Demand Active directly (stock code fails with `invalid securityHeaderFlags`; FreeRDP/Remmina accept it) |
| `ironrdp-tls`    | 0.2.2 | `upgrade_with_verifier` (stock `upgrade` accepts every certificate **and** skips handshake signature checks) |

When upgrading IronRDP, re-apply these small diffs (`grep -rn "NexDesk patch" vendor`) or upstream them.
