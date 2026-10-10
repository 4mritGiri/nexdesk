//! Operating-system backends for sharing this computer's screen and injecting input.
//! Each backend provides `Display`, `Capture` and `Injector` with the same API; the session code in
//! `host.rs` is platform independent.
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{Capture, Display, Injector};
#[cfg(all(target_os = "linux", feature = "wayland"))]
pub use linux::wayland;

#[cfg(not(target_os = "linux"))]
mod unsupported;
#[cfg(not(target_os = "linux"))]
pub use unsupported::{Capture, Display, Injector};
