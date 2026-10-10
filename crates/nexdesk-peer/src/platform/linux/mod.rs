//! Linux backends: X11 (GetImage + XTEST) and Wayland (xdg-desktop-portal ScreenCast/RemoteDesktop + PipeWire).
#[cfg(feature = "wayland")]
use std::sync::Arc;

#[cfg(feature = "wayland")]
use crate::wire::Rect;
use crate::wire::Msg;
use crate::PeerError;

mod x11;
#[cfg(feature = "wayland")]
pub mod wayland;
#[cfg(feature = "wayland")]
mod wl_capture;

use x11::{XCapture, XInjector};
#[cfg(feature = "wayland")]
use wl_capture::WlCapture;

/// The session's connection to the desktop that is shared. On Wayland this holds the portal share the
/// person approved in the desktop's own dialog; on X11 there is nothing to hold.
pub struct Display {
    #[cfg(feature = "wayland")]
    wl: Option<Arc<wayland::Wl>>,
}

impl Display {
    /// Prepare to share this desktop. On Wayland the desktop's own dialog asks the person at this computer
    /// what to share (nothing is captured before they answer). Set `NEXDESK_BACKEND=x11|wayland` to force one.
    pub fn open(view_only: bool) -> Result<Self, PeerError> {
        #[cfg(feature = "wayland")]
        {
            let want = match std::env::var("NEXDESK_BACKEND").as_deref() {
                Ok("x11") => false,
                Ok("wayland") => true,
                _ => wayland::is_wayland_session(),
            };
            let wl = if want {
                Some(wayland::Wl::start(!view_only).map_err(|e| PeerError::Capture(e))?)
            } else {
                None
            };
            Ok(Self { wl })
        }
        #[cfg(not(feature = "wayland"))]
        {
            let _ = view_only;
            Ok(Self {})
        }
    }

    pub fn capture(&self, selected: usize) -> Result<Capture, PeerError> {
        #[cfg(feature = "wayland")]
        if let Some(wl) = &self.wl {
            return WlCapture::new(wl.clone()).map(Capture::W);
        }
        XCapture::new(selected).map(Capture::X)
    }

    /// Mouse and keyboard injection, if this desktop allows it.
    pub fn injector(&self) -> Option<Injector> {
        #[cfg(feature = "wayland")]
        if let Some(w) = &self.wl {
            return Some(Injector::Wayland(w.clone()));
        }
        XInjector::new().ok().map(Injector::X)
    }
}

/// What the agent shares: an X11 screen, or a Wayland screen chosen in the desktop's own dialog.
pub enum Capture {
    X(XCapture),
    #[cfg(feature = "wayland")]
    W(WlCapture),
}

impl Capture {
    pub fn monitor_count(&self) -> usize {
        match self {
            Capture::X(c) => c.monitors.len(),
            #[cfg(feature = "wayland")]
            Capture::W(_) => 1, // the person chose one screen in the desktop's dialog
        }
    }

    pub fn selected(&self) -> usize {
        match self {
            Capture::X(c) => c.selected,
            #[cfg(feature = "wayland")]
            Capture::W(_) => 0,
        }
    }

    pub fn region(&self) -> (i16, i16, u16, u16) {
        match self {
            Capture::X(c) => (c.ox, c.oy, c.w, c.h),
            #[cfg(feature = "wayland")]
            Capture::W(c) => (0, 0, c.w, c.h),
        }
    }

    pub fn cursor(&mut self) -> Option<Msg> {
        match self {
            Capture::X(c) => c.cursor(),
            #[cfg(feature = "wayland")]
            Capture::W(_) => None, // the pointer is drawn into the picture
        }
    }

    pub fn layout_changed(&self) -> bool {
        match self {
            Capture::X(c) => c.layout_changed(),
            #[cfg(feature = "wayland")]
            Capture::W(c) => c.layout_changed(),
        }
    }

    pub fn announce(&self, view_only: bool) -> [Msg; 2] {
        match self {
            Capture::X(c) => c.announce(view_only),
            #[cfg(feature = "wayland")]
            Capture::W(c) => [
                Msg::Hello { view_only, width: c.w, height: c.h },
                Msg::Monitors { current: 0, rects: vec![Rect { x: 0, y: 0, w: c.w, h: c.h }] },
            ],
        }
    }

    pub fn next_update(&mut self) -> Result<Vec<Msg>, PeerError> {
        match self {
            Capture::X(c) => c.next_update(),
            #[cfg(feature = "wayland")]
            Capture::W(c) => c.next_update(),
        }
    }
}

/// Where input goes: XTEST on X11, or the RemoteDesktop portal on Wayland.
pub enum Injector {
    X(XInjector),
    #[cfg(feature = "wayland")]
    Wayland(Arc<wayland::Wl>),
}

impl Injector {
    #[allow(irrefutable_let_patterns)]
    pub fn set_region(&self, x: i16, y: i16, w: u16, h: u16) {
        if let Injector::X(i) = self {
            i.set_region(x, y, w, h);
        }
    }

    pub fn move_to(&self, x: u16, y: u16) -> Result<(), PeerError> {
        match self {
            Injector::X(i) => i.move_to(x, y),
            #[cfg(feature = "wayland")]
            Injector::Wayland(w) => Ok(w.move_to(x, y)),
        }
    }

    pub fn button(&self, button: u8, down: bool) -> Result<(), PeerError> {
        match self {
            Injector::X(i) => i.button(button, down),
            #[cfg(feature = "wayland")]
            Injector::Wayland(w) => Ok(w.button(button, down)),
        }
    }

    pub fn wheel(&self, dx: i16, dy: i16) -> Result<(), PeerError> {
        match self {
            Injector::X(i) => i.wheel(dx, dy),
            #[cfg(feature = "wayland")]
            Injector::Wayland(w) => Ok(w.wheel(dx, dy)),
        }
    }

    pub fn key(&self, code: u16, down: bool) -> Result<(), PeerError> {
        match self {
            Injector::X(i) => i.key(code, down),
            #[cfg(feature = "wayland")]
            Injector::Wayland(w) => Ok(w.key(code, down)),
        }
    }
}
