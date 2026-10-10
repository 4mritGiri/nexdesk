# Fuzzing

Parsers of untrusted bytes are fuzzed: peer messages, relay control messages, public identities.

```
cargo install cargo-fuzz
cargo +nightly fuzz run peer_msg_decode      # from this directory's parent
```
Add a target whenever a new parser of network input is written. Anything found goes into a regression test.
