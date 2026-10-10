//! Text clipboard sync between the two computers. Contents are never logged.
//! Linux uses the X11 selection protocol (Wayland desktops through XWayland); other systems poll the system clipboard.
#[cfg(target_os = "linux")]
mod x11;
#[cfg(target_os = "linux")]
pub use x11::ClipSync;

#[cfg(not(target_os = "linux"))]
mod portable;
#[cfg(not(target_os = "linux"))]
pub use portable::ClipSync;
