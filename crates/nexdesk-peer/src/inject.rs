//! Keyboard and mouse injection on X11 through the XTEST extension.
use std::sync::Mutex;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::Window;
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use crate::PeerError;

const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;
const MOTION: u8 = 6;

pub struct Injector {
    conn: RustConnection,
    root: Window,
    /// The shared area (x, y, w, h) in root coordinates; mouse positions are relative to it.
    region: Mutex<(i16, i16, u16, u16)>,
}

fn cap(e: impl std::fmt::Display) -> PeerError {
    PeerError::Capture(e.to_string())
}

impl Injector {
    pub fn new() -> Result<Self, PeerError> {
        let (conn, n) = x11rb::connect(None).map_err(cap)?;
        let s = &conn.setup().roots[n];
        let (root, width, height) = (s.root, s.width_in_pixels, s.height_in_pixels);
        // fail early if the extension is missing
        conn.xtest_get_version(2, 2).map_err(cap)?.reply().map_err(cap)?;
        Ok(Self { conn, root, region: Mutex::new((0, 0, width, height)) })
    }

    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) -> Result<(), PeerError> {
        self.conn.xtest_fake_input(kind, detail, 0, self.root, x, y, 0).map_err(cap)?;
        self.conn.flush().map_err(cap)?;
        Ok(())
    }

    /// Choose the monitor the viewer is looking at.
    pub fn set_region(&self, x: i16, y: i16, w: u16, h: u16) {
        if let Ok(mut r) = self.region.lock() {
            *r = (x, y, w, h);
        }
    }

    /// Move the pointer to (x, y) inside the shared area.
    pub fn move_to(&self, x: u16, y: u16) -> Result<(), PeerError> {
        let (ox, oy, w, h) = self.region.lock().map(|r| *r).unwrap_or((0, 0, 1, 1));
        let x = i32::from(ox) + i32::from(x.min(w.saturating_sub(1)));
        let y = i32::from(oy) + i32::from(y.min(h.saturating_sub(1)));
        self.fake(MOTION, 0, x.clamp(0, i32::from(i16::MAX)) as i16, y.clamp(0, i32::from(i16::MAX)) as i16)
    }

    /// 1 = left, 2 = middle, 3 = right (X11 numbering).
    pub fn button(&self, button: u8, down: bool) -> Result<(), PeerError> {
        if !(1..=3).contains(&button) {
            return Ok(());
        }
        self.fake(if down { BUTTON_PRESS } else { BUTTON_RELEASE }, button, 0, 0)
    }

    /// One click per 120 units, at least one for any non-zero delta (X11 buttons 4/5 vertical, 6/7 horizontal).
    pub fn wheel(&self, dx: i16, dy: i16) -> Result<(), PeerError> {
        let click = |btn: u8, n: i32| -> Result<(), PeerError> {
            for _ in 0..n.clamp(0, 20) {
                self.fake(BUTTON_PRESS, btn, 0, 0)?;
                self.fake(BUTTON_RELEASE, btn, 0, 0)?;
            }
            Ok(())
        };
        let steps = |d: i16| ((i32::from(d).abs() + 119) / 120).max(1);
        if dy != 0 {
            click(if dy > 0 { 4 } else { 5 }, steps(dy))?;
        }
        if dx != 0 {
            click(if dx > 0 { 7 } else { 6 }, steps(dx))?;
        }
        Ok(())
    }

    /// `code` is a Linux evdev key code; X11 keycodes are evdev + 8.
    pub fn key(&self, code: u16, down: bool) -> Result<(), PeerError> {
        let kc = u32::from(code) + 8;
        if !(8..=255).contains(&kc) {
            return Ok(()); // outside the keycode range: ignore
        }
        self.fake(if down { KEY_PRESS } else { KEY_RELEASE }, kc as u8, 0, 0)
    }
}
