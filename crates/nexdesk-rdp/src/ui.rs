//! Tiny software-rendered overlay widgets drawn on top of the remote framebuffer:
//! the full-screen connection bar (minimise / restore / close, like mstsc), modal dialogs
//! (certificate trust, errors) and toasts. No GUI toolkit: it is a handful of rectangles
//! plus an 8x8 bitmap font, which keeps the viewer dependency-light.
use std::time::{Duration, Instant};

pub const BG: u32 = 0x00_1c_21_28;
pub const BAR: u32 = 0x00_20_26_30;
pub const BAR_HOVER: u32 = 0x00_36_3f_4d;
pub const CLOSE_HOVER: u32 = 0x00_c4_2b_1c;
pub const FG: u32 = 0x00_f2_f4_f7;
pub const DIM: u32 = 0x00_9a_a5_b4;
pub const ACCENT: u32 = 0x00_3b_82_f6;
pub const WARN: u32 = 0x00_f5_9e_0b;
pub const DANGER: u32 = 0x00_ef_44_44;

pub struct Canvas<'a> {
    pub buf: &'a mut [u32],
    pub w: usize,
    pub h: usize,
}

impl Canvas<'_> {
    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        let x0 = (x.max(0) as usize).min(self.w);
        let y0 = (y.max(0) as usize).min(self.h);
        let x1 = ((x + w).max(0) as usize).min(self.w);
        let y1 = ((y + h).max(0) as usize).min(self.h);
        for yy in y0..y1 {
            self.buf[yy * self.w + x0..yy * self.w + x1.max(x0)].fill(color);
        }
    }

    pub fn frame(&mut self, x: i32, y: i32, w: i32, h: i32, t: i32, color: u32) {
        self.rect(x, y, w, t, color);
        self.rect(x, y + h - t, w, t, color);
        self.rect(x, y, t, h, color);
        self.rect(x + w - t, y, t, h, color);
    }

    /// Thick line (square brush), enough for the window-control glyphs.
    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, t: i32, color: u32) {
        let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
        for i in 0..=steps {
            let x = x0 + (x1 - x0) * i / steps;
            let y = y0 + (y1 - y0) * i / steps;
            self.rect(x - t / 2, y - t / 2, t, t, color);
        }
    }

    /// Draw ASCII text with the 8x8 font scaled by `k`; returns the drawn width.
    pub fn text(&mut self, x: i32, y: i32, s: &str, k: i32, color: u32) -> i32 {
        let mut cx = x;
        for ch in s.chars() {
            let idx = if (ch as u32) < 128 { ch as usize } else { b'?' as usize };
            let glyph = font8x8::legacy::BASIC_LEGACY[idx];
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..8 {
                    if bits >> col & 1 == 1 {
                        self.rect(cx + col * k, y + row as i32 * k, k, k, color);
                    }
                }
            }
            cx += 8 * k;
        }
        cx - x
    }
}

pub fn text_width(s: &str, k: i32) -> i32 {
    s.chars().count() as i32 * 8 * k
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_owned()
    } else {
        let mut t: String = s.chars().take(max_chars.saturating_sub(3)).collect();
        t.push_str("...");
        t
    }
}

// ------------------------------------------------------------------ connection bar

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BarHit {
    None,
    Bar,
    Pin,
    Minimize,
    Restore,
    Close,
}

pub struct Toolbar {
    pub title: String,
    pub pinned: bool,
    shown_until: Option<Instant>,
}

const HIDE_AFTER: Duration = Duration::from_millis(1800);

impl Toolbar {
    pub fn new(title: String) -> Self {
        Self { title, pinned: false, shown_until: None }
    }

    /// Keep (or make) the bar visible for a little while.
    pub fn touch(&mut self) {
        self.shown_until = Some(Instant::now() + HIDE_AFTER);
    }

    pub fn visible(&self) -> bool {
        self.pinned || self.shown_until.map(|t| Instant::now() < t).unwrap_or(false)
    }

    pub fn hide_deadline(&self) -> Option<Instant> {
        if self.pinned {
            None
        } else {
            self.shown_until.filter(|t| *t > Instant::now())
        }
    }

    pub fn hide_now(&mut self) {
        self.shown_until = None;
    }

    /// (x, y, w, h) in physical pixels. `u` is the UI scale (1 = 96 dpi).
    pub fn rect(&self, win_w: i32, u: i32) -> (i32, i32, i32, i32) {
        let h = 34 * u;
        let w = (460 * u).min(win_w);
        ((win_w - w) / 2, 0, w, h)
    }

    fn buttons(&self, win_w: i32, u: i32) -> [(BarHit, i32, i32); 4] {
        let (x, _, w, h) = self.rect(win_w, u);
        let bw = 46 * u;
        let right = x + w;
        [
            (BarHit::Pin, x, h),
            (BarHit::Minimize, right - 3 * bw, bw),
            (BarHit::Restore, right - 2 * bw, bw),
            (BarHit::Close, right - bw, bw),
        ]
    }

    pub fn hit(&self, win_w: i32, u: i32, px: f64, py: f64) -> BarHit {
        let (x, y, w, h) = self.rect(win_w, u);
        let (px, py) = (px as i32, py as i32);
        if px < x || px >= x + w || py < y || py >= y + h {
            return BarHit::None;
        }
        for (hit, bx, bw) in self.buttons(win_w, u) {
            if px >= bx && px < bx + bw {
                return hit;
            }
        }
        BarHit::Bar
    }

    pub fn draw(&self, c: &mut Canvas, u: i32, hover: BarHit) {
        let (x, y, w, h) = self.rect(c.w as i32, u);
        c.rect(x, y, w, h, BAR);
        c.rect(x, y + h - u, w, u, ACCENT);
        for (hit, bx, bw) in self.buttons(c.w as i32, u) {
            if hover == hit {
                c.rect(bx, y, bw, h - u, if hit == BarHit::Close { CLOSE_HOVER } else { BAR_HOVER });
            }
            let (cx, cy) = (bx + bw / 2, y + (h - u) / 2);
            let s = 6 * u;
            match hit {
                BarHit::Pin => {
                    // pushpin: filled when pinned, outline otherwise
                    let (px, py) = (cx - 5 * u, cy - 5 * u);
                    if self.pinned {
                        c.rect(px, py, 10 * u, 10 * u, ACCENT);
                    } else {
                        c.frame(px, py, 10 * u, 10 * u, u.max(1), FG);
                    }
                }
                BarHit::Minimize => c.rect(cx - s, cy + 2 * u, 2 * s, u.max(1) * 2, FG),
                BarHit::Restore => {
                    c.frame(cx - s + 2 * u, cy - s, 2 * s - 2 * u, 2 * s - 2 * u, u.max(1), FG);
                    c.frame(cx - s, cy - s + 2 * u, 2 * s - 2 * u, 2 * s - 2 * u, u.max(1), FG);
                }
                BarHit::Close => {
                    c.line(cx - s, cy - s, cx + s, cy + s, u.max(1) + 1, FG);
                    c.line(cx - s, cy + s, cx + s, cy - s, u.max(1) + 1, FG);
                }
                _ => {}
            }
        }
        let tx = x + 36 * u + 4 * u;
        let room = (w - 36 * u - 3 * 46 * u - 12 * u).max(0);
        let k = 2 * u.min(2) / 2 * 1;
        let k = k.max(1);
        let max_chars = (room / (8 * k)).max(0) as usize;
        c.text(tx, y + (h - 8 * k) / 2 - u, &truncate(&self.title, max_chars), k, FG);
    }
}

// ------------------------------------------------------------------ modal dialog

pub struct Modal {
    pub heading: String,
    pub lines: Vec<(String, u32)>,
    pub accent: u32,
    pub accept: Option<String>,
    pub reject: String,
    pub reply: Option<std::sync::mpsc::Sender<bool>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModalHit {
    None,
    Accept,
    Reject,
}

impl Modal {
    fn geometry(&self, win_w: i32, win_h: i32, u: i32) -> (i32, i32, i32, i32) {
        let k = 2 * u;
        let widest = self
            .lines
            .iter()
            .map(|(l, _)| text_width(l, u.max(1) * 1))
            .chain(std::iter::once(text_width(&self.heading, k)))
            .max()
            .unwrap_or(0);
        let w = (widest + 48 * u).max(420 * u).min((win_w - 20).max(1));
        let h = (70 + 14 * self.lines.len() as i32 + 70) * u;
        ((win_w - w) / 2, ((win_h - h) / 2).max(0), w, h)
    }

    fn button_rects(&self, win_w: i32, win_h: i32, u: i32) -> (Option<(i32, i32, i32, i32)>, (i32, i32, i32, i32)) {
        let (x, y, w, h) = self.geometry(win_w, win_h, u);
        let (bw, bh) = (170 * u, 32 * u);
        let by = y + h - bh - 16 * u;
        let reject = (x + w - bw - 20 * u, by, bw, bh);
        let accept = self.accept.as_ref().map(|_| (x + w - 2 * bw - 32 * u, by, bw, bh));
        (accept, reject)
    }

    pub fn hit(&self, win_w: i32, win_h: i32, u: i32, px: f64, py: f64) -> ModalHit {
        let inside = |r: (i32, i32, i32, i32)| {
            let (px, py) = (px as i32, py as i32);
            px >= r.0 && px < r.0 + r.2 && py >= r.1 && py < r.1 + r.3
        };
        let (a, r) = self.button_rects(win_w, win_h, u);
        if a.map(inside).unwrap_or(false) {
            ModalHit::Accept
        } else if inside(r) {
            ModalHit::Reject
        } else {
            ModalHit::None
        }
    }

    pub fn draw(&self, c: &mut Canvas, u: i32, hover: ModalHit) {
        // dim everything behind the dialog
        for p in c.buf.iter_mut() {
            let (r, g, b) = ((*p >> 16) & 0xff, (*p >> 8) & 0xff, *p & 0xff);
            *p = ((r / 3) << 16) | ((g / 3) << 8) | (b / 3);
        }
        let (x, y, w, h) = self.geometry(c.w as i32, c.h as i32, u);
        c.rect(x, y, w, h, BG);
        c.frame(x, y, w, h, u, self.accent);
        c.rect(x, y, w, 6 * u, self.accent);
        c.text(x + 20 * u, y + 22 * u, &self.heading, 2 * u, FG);
        for (i, (line, color)) in self.lines.iter().enumerate() {
            c.text(x + 20 * u, y + (58 + 14 * i as i32) * u, line, u.max(1), *color);
        }
        let (a, r) = self.button_rects(c.w as i32, c.h as i32, u);
        let mut button = |rect: (i32, i32, i32, i32), label: &str, primary: bool, hot: bool| {
            let bg = if primary { if hot { 0x00_60_a5_fa } else { ACCENT } } else if hot { BAR_HOVER } else { BAR };
            c.rect(rect.0, rect.1, rect.2, rect.3, bg);
            let tw = text_width(label, u.max(1));
            c.text(rect.0 + (rect.2 - tw) / 2, rect.1 + (rect.3 - 8 * u.max(1)) / 2, label, u.max(1), FG);
        };
        if let (Some(rect), Some(label)) = (a, self.accept.as_deref()) {
            button(rect, label, true, hover == ModalHit::Accept);
        }
        button(r, &self.reject, a.is_none(), hover == ModalHit::Reject);
    }
}

/// Wrap `s` into chunks of at most `n` chars (used for long fingerprints).
pub fn wrap(s: &str, n: usize) -> Vec<String> {
    s.chars().collect::<Vec<_>>().chunks(n.max(1)).map(|c| c.iter().collect()).collect()
}

// ------------------------------------------------------------------ toast

pub struct Toast {
    pub text: String,
    pub until: Instant,
}

impl Toast {
    pub fn draw(&self, c: &mut Canvas, u: i32) {
        let k = u.max(1) * 2;
        let tw = text_width(&self.text, k);
        let (w, h) = (tw + 28 * u, 16 * k + 8 * u);
        let x = (c.w as i32 - w) / 2;
        let y = c.h as i32 - h - 40 * u;
        c.rect(x, y, w, h, BAR);
        c.frame(x, y, w, h, u.max(1), ACCENT);
        c.text(x + 14 * u, y + (h - 8 * k) / 2, &self.text, k, FG);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_hit_testing() {
        let t = Toolbar::new("host".into());
        let (x, _, w, h) = t.rect(1920, 1);
        assert_eq!(t.hit(1920, 1, (x + w - 10) as f64, 5.0), BarHit::Close);
        assert_eq!(t.hit(1920, 1, (x + w - 60) as f64, 5.0), BarHit::Restore);
        assert_eq!(t.hit(1920, 1, (x + w - 110) as f64, 5.0), BarHit::Minimize);
        assert_eq!(t.hit(1920, 1, (x + 10) as f64, 5.0), BarHit::Pin);
        assert_eq!(t.hit(1920, 1, (x + w / 2) as f64, 5.0), BarHit::Bar);
        assert_eq!(t.hit(1920, 1, (x + w / 2) as f64, (h + 3) as f64), BarHit::None);
        assert_eq!(t.hit(1920, 1, 3.0, 5.0), BarHit::None);
    }

    #[test]
    fn toolbar_autohide_and_pin() {
        let mut t = Toolbar::new("h".into());
        assert!(!t.visible());
        t.touch();
        assert!(t.visible());
        t.hide_now();
        assert!(!t.visible());
        t.pinned = true;
        assert!(t.visible());
    }

    #[test]
    fn drawing_stays_inside_the_buffer() {
        let mut buf = vec![0u32; 200 * 100];
        let mut c = Canvas { buf: &mut buf, w: 200, h: 100 };
        // far outside / partially outside must not panic
        c.rect(-50, -50, 400, 400, 1);
        c.text(180, 90, "overflow text", 3, 2);
        c.line(-10, -10, 300, 300, 3, 3);
        let t = Toolbar::new("a very long host name that cannot possibly fit in this tiny window".into());
        t.draw(&mut c, 2, BarHit::Close);
        let m = Modal {
            heading: "Heading".into(),
            lines: vec![("line".into(), FG)],
            accent: WARN,
            accept: Some("Yes".into()),
            reject: "No".into(),
            reply: None,
        };
        m.draw(&mut c, 3, ModalHit::None);
    }

    #[test]
    fn modal_buttons_hit() {
        let m = Modal {
            heading: "x".into(),
            lines: vec![("a".into(), FG); 3],
            accent: WARN,
            accept: Some("Trust".into()),
            reject: "Cancel".into(),
            reply: None,
        };
        let (a, r) = m.button_rects(1000, 700, 1);
        let a = a.unwrap();
        assert_eq!(m.hit(1000, 700, 1, (a.0 + 5) as f64, (a.1 + 5) as f64), ModalHit::Accept);
        assert_eq!(m.hit(1000, 700, 1, (r.0 + 5) as f64, (r.1 + 5) as f64), ModalHit::Reject);
        assert_eq!(m.hit(1000, 700, 1, 1.0, 1.0), ModalHit::None);
    }

    #[test]
    fn wrap_chunks() {
        assert_eq!(wrap("abcdefg", 3), vec!["abc", "def", "g"]);
    }
}

#[cfg(test)]
mod preview {
    use super::*;

    /// Writes /tmp-style PNGs for eyeballing the overlays: NEXDESK_PREVIEW_DIR=dir cargo test preview
    #[test]
    fn render_previews() {
        let Some(dir) = std::env::var_os("NEXDESK_PREVIEW_DIR") else { return };
        let (w, h) = (1100usize, 620usize);
        let save = |name: &str, buf: &[u32]| {
            let mut raw = Vec::new();
            for p in buf {
                raw.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
            }
            let img = image_stub::save(&std::path::Path::new(&dir).join(name), w as u32, h as u32, &raw);
            let _ = img;
        };
        // fake "remote desktop"
        let mut buf = vec![0x00_30_60_a0u32; w * h];
        {
            let mut c = Canvas { buf: &mut buf, w, h };
            c.rect(40, 80, 500, 300, 0x00_ee_ee_ee);
            c.text(60, 100, "Remote Windows desktop", 3, 0x00_22_22_22);
            let mut t = Toolbar::new("win-server.corp.example:3389".into());
            t.pinned = true;
            t.draw(&mut c, 1, BarHit::Close);
            Toast { text: "2 file(s) ready: press Ctrl+V on the remote desktop".into(), until: Instant::now() }.draw(&mut c, 1);
        }
        save("bar.ppm", &buf);
        let mut buf = vec![BG; w * h];
        {
            let mut c = Canvas { buf: &mut buf, w, h };
            let m = Modal {
                heading: "Unknown server certificate".into(),
                lines: vec![
                    ("The identity of the remote computer could not be verified.".into(), WARN),
                    ("Connect only if you recognise the fingerprint below.".into(), DIM),
                    (String::new(), FG),
                    ("Computer : win-server.corp.example:3389".into(), FG),
                    ("Subject  : CN=WIN-SERVER".into(), FG),
                    ("Fingerprint:".into(), FG),
                    ("  sha256:0aff3b1c9d2e4f5a6b7c8d9e0f1a2b3c4d5e".into(), ACCENT),
                    ("  6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f".into(), ACCENT),
                ],
                accent: WARN,
                accept: Some("Trust and connect".into()),
                reject: "Cancel".into(),
                reply: None,
            };
            m.draw(&mut c, 1, ModalHit::Accept);
        }
        save("modal.ppm", &buf);
    }

    mod image_stub {
        pub fn save(path: &std::path::Path, w: u32, h: u32, rgb: &[u8]) {
            let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
            out.extend_from_slice(rgb);
            std::fs::write(path, out).unwrap();
        }
    }
}
