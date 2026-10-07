# Security design for NexDesk remote control (P5) and post-quantum plan

## "Quantum-level security": what is real and what is not
* **Quantum key distribution (QKD)** needs special optical hardware and fibre links. It is not something software can add. Not planned.
* **Post-quantum cryptography (PQC)** is software and is practical today. The threat it answers is *harvest now, decrypt later*: someone records
  encrypted traffic today and decrypts it when a large quantum computer exists. Remote-desktop sessions (passwords typed, files copied)
  are exactly the data worth recording.
* **What is already safe:** symmetric encryption with 256-bit keys (XChaCha20-Poly1305, AES-256-GCM) stays strong against quantum attacks
  (Grover's algorithm only halves the effective strength). The password vault (Argon2id + XChaCha20-Poly1305, 256-bit data key) already qualifies.
* **What is not safe against a future quantum computer:** key exchange and signatures built on elliptic curves / RSA (X25519, Ed25519, ECDSA, RSA).
  Those are what PQC replaces.

So "quantum-resistant" is a realistic goal for NexDesk's *own* protocol. It cannot be a promise for RDP to a Windows server, because the server chooses the TLS parameters (see "RDP mitigation").

## Design for the NexDesk protocol (P5)
| Layer | Choice | Why |
|---|---|---|
| Key exchange | **Hybrid X25519 + ML-KEM-768** (FIPS 203), secrets combined with HKDF-SHA-256 | secure if *either* algorithm holds; same pattern browsers and OpenSSH use |
| Identity / signatures | **Ed25519 + ML-DSA-65** (FIPS 204) dual signature on device identity keys | long-lived pinned keys are the part an attacker can break later |
| Session encryption | XChaCha20-Poly1305 or AES-256-GCM, per-direction keys, rekey by time and volume | quantum-safe at 256 bits |
| Hashing / KDF | SHA-256 / SHA-3, HKDF, Argon2id for passwords | |
| Handshake shape | Noise-style (known pattern, mutual authentication, forward secrecy), transcript hash bound into keys | do not invent a protocol; use a reviewed pattern with PQ KEM added |
| Pinning | device public keys pinned on first use (like `known_hosts` today) or issued by an organisation CA | |
| Server blindness | rendezvous and relay see only ciphertext and public keys | a compromised relay cannot read sessions |
| Crates | RustCrypto `ml-kem`, `ml-dsa`, `x25519-dalek`, `ed25519-dalek`, `chacha20poly1305`, `hkdf`; or `aws-lc-rs` (FIPS-validated module) | pick after a dependency audit; pin versions |

Honest limits: PQC libraries are young, so the design uses **hybrids** (never PQ-only), keeps algorithm identifiers in the handshake for agility,
and needs an independent cryptographic review before anyone relies on it. "Quantum-safe" is a property of a reviewed design plus implementation, not of the algorithm names.

## RDP mitigation (works today, no protocol change)
Wrap the RDP connection in a post-quantum tunnel: an SSH tunnel to a jump host (OpenSSH 9.0+ supports hybrid `sntrup761x25519-sha512`; 10.0+ defaults to `mlkem768x25519-sha256`),
or WireGuard with a pre-shared key (adds symmetric, quantum-resistant protection to the handshake). NexDesk P2 "SSH tunnel" + pre/post commands (done) can drive this;
the connection editor will get a "Tunnel through SSH host" option. The inner RDP/TLS session is then no longer decryptable from recorded network traffic alone.
Also P1: refuse legacy RDP security and require TLS/NLA.

## Enterprise-grade controls (P5/P6)
* **Consent by default**: incoming connections need an explicit accept unless an admin policy enables unattended access for named identities.
* **Strong authentication**: TOTP, FIDO2/WebAuthn security keys, device certificates (mTLS) issued by the organisation; no shared "permanent password" as the only factor.
* **Least privilege per session**: view-only, clipboard, files, audio, tunnels, recording are separate permissions, each default-deny, set by policy file/server.
* **Tamper-evident audit log**: hash-chained entries, signed, shipped to a SIEM (syslog/OTLP); logs never contain passwords, clipboard text or file contents (already true locally).
* **Admin policy**: signed policy file/server push (allowed IDs, forced relay, forbidden features, minimum version), enforced in the agent not just the UI.
* **Supply chain**: signed releases and updates, SBOM, `cargo-audit`/`cargo-deny` in CI, reproducible builds, pinned dependencies.
* **Hardening**: memory-safe Rust only in network paths, fuzzing of the protocol parser, privilege separation (agent service vs UI), sandbox the file-transfer staging area (already confined).
* **Independent review**: third-party penetration test and cryptographic review before the first non-preview release.

## What this changes in the plan
* P1: TLS/NLA-required flag, zero-trust profile (already planned).
* P2: SSH tunnel for RDP (also the post-quantum path for RDP), WireGuard-PSK guidance in docs.
* P5a: hybrid handshake `nexdesk-crypto` is the *first* crate written, with known-answer tests, tamper tests and a fuzz target, before capture/codec work.
* P5c: relay/rendezvous see ciphertext only; 2FA and device certificates.
* P6: audit/SIEM, policy server, SSO. Review gate before release.
