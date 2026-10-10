//! X11 backend: the screen is read with GetImage (XDamage and XFixes when present), input is injected with XTEST.
use std::sync::Mutex;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, Window};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use crate::frame::{dirty_rects, extract};
use crate::wire::{pack_pixels, Msg, Rect, MAX_MONITORS};
use crate::PeerError;

fn cap(e: impl std::fmt::Display) -> PeerError {
    PeerError::Capture(e.to_string())
}



const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;
const MOTION: u8 = 6;

pub struct XInjector {
    conn: RustConnection,
    root: Window,
    /// The shared area (x, y, w, h) in root coordinates; mouse positions are relative to it.
    region: Mutex<(i16, i16, u16, u16)>,
}


impl XInjector {
    pub fn new() -> Result<Self, PeerError> {
        let (conn, n) = x11rb::connect(None).map_err(cap)?;
        let s = &conn.setup().roots[n];
        let (root, width, height) = (s.root, s.width_in_pixels, s.height_in_pixels);
        // fail early if the extension is missing
        conn.xtest_get_version(2, 2)
            .map_err(cap)?
            .reply()
            .map_err(cap)?;
        Ok(Self {
            conn,
            root,
            region: Mutex::new((0, 0, width, height)),
        })
    }

    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) -> Result<(), PeerError> {
        self.conn
            .xtest_fake_input(kind, detail, 0, self.root, x, y, 0)
            .map_err(cap)?;
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
        self.fake(
            MOTION,
            0,
            x.clamp(0, i32::from(i16::MAX)) as i16,
            y.clamp(0, i32::from(i16::MAX)) as i16,
        )
    }

    /// 1 = left, 2 = middle, 3 = right (X11 numbering).
    pub fn button(&self, button: u8, down: bool) -> Result<(), PeerError> {
        if !(1..=3).contains(&button) {
            return Ok(());
        }
        self.fake(
            if down { BUTTON_PRESS } else { BUTTON_RELEASE },
            button,
            0,
            0,
        )
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


pub struct XCapture {
    conn: RustConnection,
    root: u32,
    /// The shared area (one monitor, or the whole screen) in root coordinates.
    pub(super) ox: i16,
    pub(super) oy: i16,
    pub(super) w: u16,
    pub(super) h: u16,
    /// Whole X screen size and monitor list when this capture was set up.
    root_size: (u16, u16),
    pub(super) monitors: Vec<Rect>,
    pub(super) selected: usize,
    prev: Vec<u8>,
    xfixes: bool,
    cursor_serial: u32,
    damage: Option<u32>,
    last_grab: Instant,
}

/// Monitors from RandR; the whole screen when RandR is missing or lists nothing usable.
fn list_monitors(conn: &RustConnection, root: u32, root_w: u16, root_h: u16) -> Vec<Rect> {
    use x11rb::protocol::randr::ConnectionExt as _;
    let mut out: Vec<Rect> = conn
        .randr_get_monitors(root, true)
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| {
            r.monitors
                .iter()
                .filter(|m| m.width > 0 && m.height > 0)
                .map(|m| Rect {
                    x: m.x,
                    y: m.y,
                    w: m.width,
                    h: m.height,
                })
                // keep only monitors inside the screen, so a grab can never fail on bounds
                .filter(|m| {
                    m.x >= 0
                        && m.y >= 0
                        && m.x as u32 + m.w as u32 <= root_w as u32
                        && m.y as u32 + m.h as u32 <= root_h as u32
                })
                .collect()
        })
        .unwrap_or_default();
    out.truncate(MAX_MONITORS);
    if out.is_empty() {
        out.push(Rect {
            x: 0,
            y: 0,
            w: root_w,
            h: root_h,
        });
    }
    out
}

impl XCapture {
    pub(super) fn new(selected: usize) -> Result<Self, PeerError> {
        let (conn, n) = x11rb::connect(None).map_err(cap)?;
        let setup = conn.setup();
        let s = &setup.roots[n];
        let fmt = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == s.root_depth)
            .ok_or_else(|| cap("no pixmap format for the root depth"))?;
        if fmt.bits_per_pixel != 32 {
            return Err(cap(format!(
                "unsupported screen format ({} bits per pixel)",
                fmt.bits_per_pixel
            )));
        }
        let root = s.root;
        let (root_w, root_h) = (s.width_in_pixels, s.height_in_pixels);
        let xfixes = x11rb::protocol::xfixes::query_version(&conn, 5, 0)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some();
        let monitors = list_monitors(&conn, root, root_w, root_h);
        let selected = selected.min(monitors.len() - 1);
        let m = monitors[selected];
        // XDamage tells us when anything on screen changed, so an idle screen costs nothing
        let damage = (|| {
            use x11rb::protocol::damage::{ConnectionExt as _, ReportLevel};
            conn.damage_query_version(1, 1).ok()?.reply().ok()?;
            let id = conn.generate_id().ok()?;
            conn.damage_create(id, root, ReportLevel::NON_EMPTY)
                .ok()?
                .check()
                .ok()?;
            Some(id)
        })();
        Ok(Self {
            conn,
            root,
            ox: m.x,
            oy: m.y,
            w: m.w,
            h: m.h,
            root_size: (root_w, root_h),
            monitors,
            selected,
            prev: Vec::new(),
            xfixes,
            cursor_serial: 0,
            damage,
            last_grab: Instant::now(),
        })
    }

    pub(super) fn grab(&self) -> Result<Vec<u8>, PeerError> {
        let r = self
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.root,
                self.ox,
                self.oy,
                self.w,
                self.h,
                !0,
            )
            .map_err(cap)?
            .reply()
            .map_err(cap)?;
        if r.data.len() != self.w as usize * self.h as usize * 4 {
            return Err(cap("unexpected image size"));
        }
        Ok(r.data)
    }

    /// A new pointer image if it changed since the last call.
    pub(super) fn cursor(&mut self) -> Option<Msg> {
        if !self.xfixes {
            return None;
        }
        let r = x11rb::protocol::xfixes::get_cursor_image(&self.conn)
            .ok()?
            .reply()
            .ok()?;
        if r.cursor_serial == self.cursor_serial {
            return None;
        }
        self.cursor_serial = r.cursor_serial;
        let (w, h) = (r.width, r.height);
        if w == 0
            || h == 0
            || w > crate::wire::MAX_CURSOR
            || h > crate::wire::MAX_CURSOR
            || r.cursor_image.len() != w as usize * h as usize
        {
            return None;
        }
        let bytes: Vec<u8> = r
            .cursor_image
            .iter()
            .flat_map(|p| p.to_le_bytes())
            .collect();
        Some(Msg::Cursor {
            hot_x: r.xhot.min(w - 1),
            hot_y: r.yhot.min(h - 1),
            w,
            h,
            lz4: pack_pixels(&bytes),
        })
    }

    /// The screen size or the monitor layout is not what we set up with.
    pub(super) fn layout_changed(&self) -> bool {
        match self
            .conn
            .get_geometry(self.root)
            .ok()
            .and_then(|c| c.reply().ok())
        {
            Some(g) => {
                (g.width, g.height) != self.root_size
                    || list_monitors(&self.conn, self.root, g.width, g.height) != self.monitors
            }
            None => false,
        }
    }

    /// Messages that tell the viewer what it is looking at.
    pub(super) fn announce(&self, view_only: bool) -> [Msg; 2] {
        [
            Msg::Hello {
                view_only,
                width: self.w,
                height: self.h,
            },
            Msg::Monitors {
                current: self.selected as u8,
                rects: self.monitors.clone(),
            },
        ]
    }

    /// True when something on the screen changed since the last grab (always true without XDamage,
    /// and at least once a second as a safety net).
    pub(super) fn needs_grab(&mut self) -> bool {
        let Some(d) = self.damage else { return true };
        let mut dirty = self.prev.is_empty() || self.last_grab.elapsed() >= Duration::from_secs(1);
        while let Ok(Some(ev)) = self.conn.poll_for_event() {
            if matches!(ev, x11rb::protocol::Event::DamageNotify(_)) {
                dirty = true;
            }
        }
        if dirty {
            use x11rb::protocol::damage::ConnectionExt as _;
            // re-arm: the next change produces a new notification
            let _ = self.conn.damage_subtract(d, 0u32, 0u32);
            let _ = self.conn.flush();
        }
        dirty
    }

    /// Tiles that changed since the last call, merged per tile row, as ready-to-send messages.
    pub(super) fn next_update(&mut self) -> Result<Vec<Msg>, PeerError> {
        if !self.needs_grab() {
            return Ok(Vec::new());
        }
        self.last_grab = Instant::now();
        let cur = self.grab()?;
        let rects = dirty_rects(&self.prev, &cur, self.w as usize, self.h as usize);
        let msgs = rects
            .into_iter()
            .map(|(x, y, w, h)| Msg::Tile {
                x: x as u16,
                y: y as u16,
                w: w as u16,
                h: h as u16,
                lz4: pack_pixels(&extract(&cur, self.w as usize, (x, y, w, h))),
            })
            .collect();
        self.prev = cur;
        Ok(msgs)
    }
}

