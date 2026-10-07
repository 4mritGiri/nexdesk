//! NexDesk remote-control protocol (P5a prototype): a host agent that shares an X11 screen and a viewer.
//!
//! Everything after the handshake travels inside `nexdesk-crypto` records. See `docs/PEER.md`.
pub mod client;
pub mod clip;
pub mod control;
pub mod host;
pub mod inject;
pub mod link;
pub mod screen;
pub mod store;
pub mod wire;

pub use link::{Reader, Writer};
pub use wire::Msg;

#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    #[error("network error: {0}")]
    Io(#[from] std::io::Error),
    #[error("secure channel: {0}")]
    Crypto(#[from] nexdesk_crypto::Error),
    #[error("protocol error: {0}")]
    Proto(&'static str),
    #[error("connection closed")]
    Closed,
    #[error("screen capture: {0}")]
    Capture(String),
}

/// Default TCP port of the agent.
pub const DEFAULT_PORT: u16 = 21118;
