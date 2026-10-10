//! Abstraction over "the local desktop's clipboard" so the RDP-side logic in `engine.rs`
//! does not care whether it talks to X11 (also used for Wayland sessions through XWayland)
//! or another windowing system.
use std::sync::Arc;

/// One-shot answer to a data request: `None` = cannot provide.
pub type Reply = Box<dyn FnOnce(Option<Vec<u8>>) + Send>;

/// Supplies clipboard data lazily when a local application pastes.
/// `target` is the MIME type / X11 target name that was requested.
pub trait Provider: Send + Sync {
    fn request(&self, target: &str, reply: Reply);
}

#[derive(Debug)]
pub enum TransportEvent {
    /// Another application took the clipboard; `targets` are the formats it offers
    /// (empty = the clipboard has no owner / is empty).
    OwnerChanged(Vec<String>),
    /// Result of [`Transport::read`].
    ReadDone {
        token: u64,
        result: Result<Vec<u8>, String>,
    },
}

pub type EventSink = Arc<dyn Fn(TransportEvent) + Send + Sync>;

pub trait Transport: Send + Sync {
    /// Fetch the clipboard content in `target`; the result arrives as `ReadDone{token}`.
    fn read(&self, token: u64, target: &str);
    /// Become the clipboard owner offering `targets`, served lazily through `provider`.
    fn own(&self, targets: Vec<String>, provider: Arc<dyn Provider>);
    /// Give the clipboard up (if we still own it).
    fn disown(&self);
    /// Re-announce the current owner's formats as `OwnerChanged`.
    fn query(&self);
    fn name(&self) -> &'static str;
}
