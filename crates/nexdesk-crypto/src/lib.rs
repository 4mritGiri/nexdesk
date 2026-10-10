//! Hybrid post-quantum handshake and secure channel for the NexDesk remote-control protocol.
//!
//! **Status: prototype, not independently reviewed.** Do not rely on it for real secrets before the
//! review gate in `docs/security/SECURITY_DESIGN.md`. The underlying ML-KEM / ML-DSA crates also state that
//! they have never been audited.
//!
//! * Key exchange: X25519 **and** ML-KEM-768, secrets combined with HKDF-SHA-256 (secure if either holds).
//! * Identity: Ed25519 **and** ML-DSA-65; both signatures must verify.
//! * Handshake: three messages, mutual authentication, transcript bound into every key, forward secrecy
//!   (ephemeral keys only), identities sent encrypted.
//! * Channel: ChaCha20-Poly1305 with per-direction keys, strict in-order counters (replay / reorder / drop
//!   are errors) and periodic key ratcheting.
//!
//! The caller decides whom to trust: the handshake hands over the peer's [`IdentityPublic`] (and its
//! fingerprint) and only continues if the supplied closure accepts it (pinned key, allow-list, prompt).
mod channel;
mod handshake;
mod identity;

pub use channel::{Opener, Sealer, Session};
pub use handshake::{Initiator, InitiatorWaiting, Responder, ResponderWaiting};
pub use identity::{Identity, IdentityPublic};

/// Errors never say *why* a verification failed beyond the coarse category, to avoid oracles.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("malformed message")]
    Malformed,
    #[error("unsupported protocol version")]
    Version,
    #[error("authentication failed")]
    Auth,
    #[error("peer rejected by policy")]
    Rejected,
    #[error("message too large")]
    TooLarge,
    #[error("replayed, reordered or corrupted record")]
    Record,
    #[error("session exhausted, reconnect")]
    Exhausted,
    #[error("randomness unavailable")]
    Rng,
}

/// Largest single record accepted by the channel (16 MiB).
pub const MAX_RECORD: usize = 16 * 1024 * 1024;
