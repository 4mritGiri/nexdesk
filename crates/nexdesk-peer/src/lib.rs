//! NexDesk remote-control protocol (P5a prototype): a host agent that shares an X11 screen and a viewer.
//!
//! Everything after the handshake travels inside `nexdesk-crypto` records. See `docs/architecture/PEER.md`.
pub mod agent;
pub mod client;
pub mod clip;
pub mod control;
pub mod frame;
pub mod host;
pub mod keymap;
pub mod link;
pub mod local;
pub mod overlay;
pub mod paths;
pub mod platform;
pub mod screen;
pub mod shot;
pub mod store;
pub mod viewer;
pub mod wire;
pub mod xfer;

pub use link::{Reader, Writer};
#[cfg(all(target_os = "linux", feature = "wayland"))]
pub use platform::wayland;
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
    #[error("{0}")]
    Relay(&'static str),
    #[error("screen capture: {0}")]
    Capture(String),
}

/// Default TCP port of the agent.
pub const DEFAULT_PORT: u16 = 21118;
