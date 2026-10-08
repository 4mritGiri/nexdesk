//! Rendezvous and relay for NexDesk native remote control.
//!
//! An agent behind NAT keeps one control connection open to the relay under a 9 digit ID. A viewer asks the
//! relay for that ID; the relay tells the agent, the agent opens a second connection, and the relay then copies
//! bytes between the two connections. The relay never sees plaintext: the viewer and the agent run the
//! `nexdesk-crypto` handshake *through* it, so a malicious relay can refuse or drop traffic but cannot read,
//! change or impersonate anything. See `docs/RELAY.md`.
pub mod client;
pub mod proto;
pub mod relay;

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("network error: {0}")]
    Io(#[from] std::io::Error),
    #[error("relay protocol error: {0}")]
    Proto(&'static str),
    #[error("{0}")]
    Refused(&'static str),
}

/// Default relay port.
pub const DEFAULT_RELAY_PORT: u16 = 21117;
