# nexdesk-crypto (P5a, step 1)

Hybrid post-quantum handshake and record layer for the NexDesk remote-control protocol.
**Prototype. Not independently reviewed. The ML-KEM and ML-DSA crates it uses state that they have never been audited.**
Do not protect real secrets with it before the review gate in `docs/security/SECURITY_DESIGN.md`.

## Messages
```
I -> R  msg1: "NDH1" | x25519_eph_I | mlkem768_ek_I | nonce_I                       (1252 bytes)
R -> I  msg2: "NDH2" | x25519_eph_R | mlkem768_ct   | nonce_R | AEAD(identity_R, sig_R(th1))
I -> R  msg3: "NDH3" | AEAD(identity_I, sig_I(th2))
```
* Key schedule input: `x25519_secret || mlkem_secret` through HKDF-SHA-256, salted with the transcript hash. Both must be broken to recover keys.
* Identity = Ed25519 + ML-DSA-65. A signature counts only if **both** verify. Signatures are domain-separated per role and bound to the transcript.
* Identities are sent encrypted (passive observers cannot tell who connects). The responder authenticates first; the initiator's identity is revealed only after the responder is verified.
* The caller supplies `accept_peer(&IdentityPublic) -> bool` on both sides: pinned fingerprint, allow-list or a consent prompt. Fingerprints print as `SHA256:xxxxxxxx-...`.
* Records: ChaCha20-Poly1305, one key per direction, counter nonce (not sent), strict in-order delivery, key ratchet every 2^20 records, 16 MiB record limit.
* Low-order X25519 points are refused; malformed lengths and versions are rejected before any crypto work.

## What the tests cover (`cargo test -p nexdesk-crypto`, about 15 s in debug)
round trip both directions; flipping sampled bytes of every handshake message always fails; rejected peers stop the handshake on both sides;
impostor responder is caught by the pin check; replay, reordering, tampering, wrong AAD and reflection of records fail; rekey boundary stays in sync;
single-algorithm signature (Ed25519 only or ML-DSA only) is not accepted; role separation; seed/fingerprint round trip.

## Not done yet (do before relying on it)
* Known-answer tests against NIST ML-KEM / ML-DSA vectors and a fuzz target for the message parsers.
* Hedged (randomised) ML-DSA signing; the prototype uses the deterministic variant.
* Constant-time review, zeroisation of ML-KEM secrets (the dependency does not guarantee it), side-channel review.
* Algorithm agility / version negotiation, session resumption, and an identity store (seeds belong in the vault).
* Independent cryptographic review and a second implementation cross-check.
