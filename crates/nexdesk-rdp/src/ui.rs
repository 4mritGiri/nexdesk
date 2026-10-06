//! Tiny software-rendered overlay widgets drawn on top of the remote framebuffer:
//! the full-screen connection bar (minimise / restore / close, like mstsc), modal dialogs
//! (certificate trust, errors) and toasts. No GUI toolkit: it is a handful of rectangles
//! plus an 8x8 bitmap font, which keeps the viewer dependency-light.
use std::time::{Duration, Instant};

// GNOME / libadwaita dark palette (matches the Files app look).
pub const BG: u32 = 0x00_1e_1e_1e;
pub const BAR: u32 = 0x00_2b_2b_2b;
pub const BAR_HOVER: u32 = 0x00_3d_3d_3d;
pub const FG: u32 = 0x00_f0_f0_f0;
pub const DIM: u32 = 0x00_9a_99_96;
pub const ACCENT: u32 = 0x00_35_84_e4;
pub const ACCENT_HI: u32 = 0x00_62_a0_ea;
pub const WARN: u32 = 0x00_f5_c2_11;
pub const DANGER: u32 = 0x00_ff_7b_63;
pub const EDGE: u32 = 0x00_44_44_44;
const RED: u32 = 0x00_ff_5f_57;
const YELLOW: u32 = 0x00_fe_bc_2e;
const GREEN: u32 = 0x00_28_c8_40;
const DOT_OFF: u32 = 0x00_55_55_55;

fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = (((a >> s) & 0xff) as f32, ((b >> s) & 0xff) as f32);
        ((x + (y - x) * t).round() as u32) & 0xff
    };
    ch(16) << 16 | ch(8) << 8 | ch(0)
}

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

    /// Rounded rectangle with anti-aliased corners. `corners` = which corners are round
    /// (top-left, top-right, bottom-right, bottom-left).
    pub fn rrect_corners(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, color: u32, corners: [bool; 4]) {
        let r = r.min(w / 2).min(h / 2).max(0);
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w).min(self.w as i32);
        let y1 = (y + h).min(self.h as i32);
        for yy in y0..y1 {
            for xx in x0..x1 {
                let (lx, ly) = (xx - x, yy - y);
                // Which corner circle (if any) is this pixel in?
                let cx = if lx < r { Some((r, 0usize, 3usize)) } else if lx >= w - r { Some((w - r, 1, 2)) } else { None };
                let cy = if ly < r { Some(r) } else if ly >= h - r { Some(h - r) } else { None };
                let mut cov = 1.0f32;
                if let (Some((ccx, top_idx, bot_idx)), Some(ccy)) = (cx, cy) {
                    let corner = if ly < r { top_idx } else { bot_idx };
                    if corners[corner] {
                        let dx = xx as f32 + 0.5 - (x + ccx) as f32;
                        let dy = yy as f32 + 0.5 - (y + ccy) as f32;
                        let d = (dx * dx + dy * dy).sqrt();
                        cov = (r as f32 - d + 0.5).clamp(0.0, 1.0);
                    }
                }
                if cov >= 0.999 {
                    self.buf[yy as usize * self.w + xx as usize] = color;
                } else if cov > 0.0 {
                    let i = yy as usize * self.w + xx as usize;
                    self.buf[i] = mix(self.buf[i], color, cov);
                }
            }
        }
    }

    pub fn rrect(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, color: u32) {
        self.rrect_corners(x, y, w, h, r, color, [true; 4]);
    }

    pub fn circle(&mut self, cx: i32, cy: i32, r: i32, color: u32) {
        self.rrect(cx - r, cy - r, 2 * r, 2 * r, r, color);
    }

    /// Rounded rectangle outline.
    pub fn rframe(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, t: i32, border: u32, fill: u32) {
        self.rrect(x, y, w, h, r, border);
        self.rrect(x + t, y + t, w - 2 * t, h - 2 * t, (r - t).max(0), fill);
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
    /// Send Ctrl+Alt+Del to the remote machine.
    Cad,
    /// Save a PNG of the remote screen.
    Shot,
    /// Toggle fit-to-window / 1:1.
    Scale,
    /// Freeze the view and block input.
    Pause,
}

/// Floating pill at the top centre of the full-screen session, styled like the GNOME Files
/// header: traffic-light window buttons on the left, host name, session tools on the right.
pub struct Toolbar {
    pub title: String,
    pub pinned: bool,
    /// Docked = permanent full-width header (windowed mode) instead of the floating full-screen pill.
    pub docked: bool,
    /// View is frozen and input is not forwarded.
    pub paused: bool,
    /// 1:1 (pannable) view instead of fit-to-window.
    pub actual: bool,
    shown_until: Option<Instant>,
}

const HIDE_AFTER: Duration = Duration::from_millis(1800);
const TOOL_STEP: i32 = 32;

impl Toolbar {
    pub fn new(title: String) -> Self {
        Self { title, pinned: false, docked: false, paused: false, actual: false, shown_until: None }
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
        let h = 40 * u;
        if self.docked {
            return (0, 0, win_w, h);
        }
        let w = (540 * u).min(win_w);
        ((win_w - w) / 2, 0, w, h)
    }

    /// Hit areas as (kind, centre-x, half-width), left to right.
    fn buttons(&self, win_w: i32, u: i32) -> Vec<(BarHit, i32, i32)> {
        let (x, _, w, _) = self.rect(win_w, u);
        let step = 28 * u;
        let c0 = x + 26 * u;
        let mut v = vec![
            (BarHit::Close, c0, 14 * u),
            (BarHit::Minimize, c0 + step, 14 * u),
            (BarHit::Restore, c0 + 2 * step, 14 * u),
        ];
        // Session tools sit on the right; the pin (floating bar only) is the last one.
        let last = if self.docked { x + w - 22 * u } else { x + w - 26 * u - TOOL_STEP * u };
        let tools = [BarHit::Cad, BarHit::Shot, BarHit::Scale, BarHit::Pause];
        for (i, hit) in tools.iter().enumerate() {
            v.push((*hit, last - (3 - i as i32) * TOOL_STEP * u, TOOL_STEP / 2 * u - u));
        }
        if !self.docked {
            v.push((BarHit::Pin, x + w - 26 * u, 16 * u));
        }
        v
    }

    pub fn hit(&self, win_w: i32, u: i32, px: f64, py: f64) -> BarHit {
        let (x, y, w, h) = self.rect(win_w, u);
        let (px, py) = (px as i32, py as i32);
        if px < x || px >= x + w || py < y || py >= y + h {
            return BarHit::None;
        }
        for (hit, cx, half) in self.buttons(win_w, u) {
            if (px - cx).abs() <= half {
                return hit;
            }
        }
        BarHit::Bar
    }

    /// Short hover text for a button.
    pub fn tip(&self, hit: BarHit) -> Option<&'static str> {
        Some(match hit {
            BarHit::Close => "Disconnect",
            BarHit::Minimize => "Minimize",
            BarHit::Restore => "Toggle full screen",
            BarHit::Pin => if self.pinned { "Unpin bar" } else { "Pin bar" },
            BarHit::Cad => "Send Ctrl+Alt+Del",
            BarHit::Shot => "Save screenshot",
            BarHit::Scale => if self.actual { "Fit to window" } else { "Actual size (1:1)" },
            BarHit::Pause => if self.paused { "Resume" } else { "Pause" },
            BarHit::None | BarHit::Bar => return None,
        })
    }

    pub fn draw(&self, c: &mut Canvas, u: i32, hover: BarHit) {
        let (x, y, w, h) = self.rect(c.w as i32, u);
        if self.docked {
            c.rect(x, y, w, h, BAR);
            c.rect(x, y + h - u, w, u, EDGE);
        } else {
            // hangs from the top edge: only the bottom corners are round
            c.rrect_corners(x - u, y, w + 2 * u, h + u, 16 * u, EDGE, [false, false, true, true]);
            c.rrect_corners(x, y, w, h, 15 * u, BAR, [false, false, true, true]);
        }
        let cy = y + h / 2;
        let buttons = self.buttons(c.w as i32, u);
        for &(hit, cx, _) in &buttons {
            let hot = hover == hit;
            match hit {
                BarHit::Close | BarHit::Minimize | BarHit::Restore => {
                    let (col, glyph_col) = match hit {
                        BarHit::Close => (RED, 0x00_5c_10_0c),
                        BarHit::Minimize => (YELLOW, 0x00_6b_4b_00),
                        _ => (GREEN, 0x00_0b_55_16),
                    };
                    c.circle(cx, cy, 7 * u, col);
                    if hot {
                        let g = 3 * u;
                        let t = u.max(1);
                        match hit {
                            BarHit::Close => {
                                c.line(cx - g, cy - g, cx + g, cy + g, t, glyph_col);
                                c.line(cx - g, cy + g, cx + g, cy - g, t, glyph_col);
                            }
                            BarHit::Minimize => c.rect(cx - g, cy - t / 2, 2 * g, t, glyph_col),
                            _ => {
                                c.frame(cx - g, cy - g, 2 * g, 2 * g, t, glyph_col);
                            }
                        }
                    }
                }
                BarHit::Pin => {
                    // round button, filled accent when pinned
                    if hot {
                        c.circle(cx, cy, 13 * u, BAR_HOVER);
                    }
                    if self.pinned {
                        c.circle(cx, cy, 6 * u, ACCENT_HI);
                    } else {
                        c.circle(cx, cy, 6 * u, DOT_OFF);
                        c.circle(cx, cy, 4 * u, if hot { BAR_HOVER } else { BAR });
                    }
                }
                _ => {
                    let on = match hit {
                        BarHit::Scale => self.actual,
                        BarHit::Pause => self.paused,
                        _ => false,
                    };
                    if hot {
                        c.rrect(cx - 14 * u, cy - 14 * u, 28 * u, 28 * u, 8 * u, BAR_HOVER);
                    } else if on {
                        c.rrect(cx - 14 * u, cy - 14 * u, 28 * u, 28 * u, 8 * u, mix(BAR, ACCENT, 0.35));
                    }
                    let col = if on { ACCENT_HI } else if hot { FG } else { DIM };
                    draw_tool_icon(c, hit, cx, cy, u, col, self.actual, self.paused);
                }
            }
        }
        let k = u.max(1);
        let left = x + 26 * u + 3 * 28 * u - 4 * u;
        let tools_left = buttons
            .iter()
            .filter(|b| matches!(b.0, BarHit::Cad | BarHit::Shot | BarHit::Scale | BarHit::Pause | BarHit::Pin))
            .map(|b| b.1 - b.2)
            .min()
            .unwrap_or(x + w);
        let right = tools_left - 6 * u;
        let room = (right - left).max(0);
        let max_chars = (room / (8 * k)).max(0) as usize;
        let mut label = self.title.clone();
        if self.paused {
            label = format!("{} (paused)", label);
        }
        let title = truncate(&label, max_chars);
        let tw = text_width(&title, k);
        c.text(left + (room - tw) / 2, cy - 4 * k, &title, k, if self.paused { WARN } else { FG });

        // Tooltip under the hovered button.
        if let Some(tip) = self.tip(hover) {
            if let Some(&(_, cx, _)) = buttons.iter().find(|b| b.0 == hover) {
                let tw = text_width(tip, k);
                let (bw, bh) = (tw + 16 * u, 22 * u);
                let bx = (cx - bw / 2).clamp(2 * u, (c.w as i32 - bw - 2 * u).max(2 * u));
                let by = y + h + 6 * u;
                c.rrect(bx - u, by - u, bw + 2 * u, bh + 2 * u, 7 * u, EDGE);
                c.rrect(bx, by, bw, bh, 6 * u, BG);
                c.text(bx + 8 * u, by + (bh - 8 * k) / 2, tip, k, FG);
            }
        }
    }
}

/// Simple glyphs for the session tools, built from rectangles/lines so they stay crisp at any scale.
fn draw_tool_icon(c: &mut Canvas, hit: BarHit, cx: i32, cy: i32, u: i32, col: u32, actual: bool, paused: bool) {
    let t = u.max(1);
    match hit {
        BarHit::Cad => {
            let k = u.max(1);
            let tw = text_width("DEL", k);
            c.text(cx - tw / 2, cy - 4 * k, "DEL", k, col);
        }
        BarHit::Shot => {
            // camera: body, viewfinder bump and lens
            c.rrect(cx - 8 * u, cy - 5 * u, 16 * u, 11 * u, 2 * u, col);
            c.rect(cx - 3 * u, cy - 7 * u, 6 * u, 3 * u, col);
            c.circle(cx, cy + u, 3 * u + u / 2, BAR);
            c.circle(cx, cy + u, 2 * u, col);
        }
        BarHit::Scale => {
            if actual {
                // currently 1:1 -> show the "shrink to fit" glyph
                c.frame(cx - 8 * u, cy - 6 * u, 16 * u, 12 * u, t, col);
                c.rect(cx - 3 * u, cy - 2 * u, 6 * u, 4 * u, col);
            } else {
                // corner brackets = "expand to actual size"
                let l = 4 * u;
                for (sx, sy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
                    let (px, py) = (cx + sx * 8 * u, cy + sy * 6 * u);
                    c.rect(px.min(px - sx * l), py - t / 2, l, t, col);
                    c.rect(px - t / 2, py.min(py - sy * l), t, l, col);
                }
            }
        }
        BarHit::Pause => {
            if paused {
                // play triangle
                for i in 0..(10 * u) {
                    let half = (10 * u - i) * 6 * u / (10 * u);
                    c.rect(cx - 4 * u + i / 1, cy - half, 1, 2 * half.max(1), col);
                }
            } else {
                c.rrect(cx - 5 * u, cy - 6 * u, 3 * u + u / 2, 12 * u, u, col);
                c.rrect(cx + u + u / 2, cy - 6 * u, 3 * u + u / 2, 12 * u, u, col);
            }
        }
        _ => {}
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
            *p = mix(*p, 0, 0.62);
        }
        let (x, y, w, h) = self.geometry(c.w as i32, c.h as i32, u);
        c.rrect(x - u, y - u, w + 2 * u, h + 2 * u, 15 * u, EDGE);
        c.rrect(x, y, w, h, 14 * u, BAR);
        c.text(x + 22 * u, y + 24 * u, &self.heading, 2 * u, if self.accent == DANGER { DANGER } else { FG });
        for (i, (line, color)) in self.lines.iter().enumerate() {
            c.text(x + 22 * u, y + (62 + 14 * i as i32) * u, line, u.max(1), *color);
        }
        let (a, r) = self.button_rects(c.w as i32, c.h as i32, u);
        let danger = self.accent == DANGER;
        let mut button = |rect: (i32, i32, i32, i32), label: &str, primary: bool, hot: bool| {
            let bg = match (primary, danger, hot) {
                (true, true, _) => if hot { 0x00_f6_61_51 } else { 0x00_c0_1c_28 },
                (true, false, true) => ACCENT_HI,
                (true, false, false) => ACCENT,
                (false, _, true) => 0x00_4a_4a_4a,
                (false, _, false) => 0x00_3a_3a_3a,
            };
            c.rrect(rect.0, rect.1, rect.2, rect.3, 9 * u, bg);
            let tw = text_width(label, u.max(1));
            c.text(rect.0 + (rect.2 - tw) / 2, rect.1 + (rect.3 - 8 * u.max(1)) / 2, label, u.max(1), 0x00_ff_ff_ff);
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
        let (w, h) = (tw + 36 * u, 16 * k + 6 * u);
        let x = (c.w as i32 - w) / 2;
        let y = c.h as i32 - h - 40 * u;
        c.rrect(x - u, y - u, w + 2 * u, h + 2 * u, h / 2 + u, EDGE);
        c.rrect(x, y, w, h, h / 2, BAR);
        c.text(x + 18 * u, y + (h - 8 * k) / 2, &self.text, k, FG);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_hit_testing() {
        let t = Toolbar::new("host".into());
        let (x, _, w, h) = t.rect(1920, 1);
        // traffic lights on the left (close, minimise, restore), pin on the right
        assert_eq!(t.hit(1920, 1, (x + 26) as f64, 20.0), BarHit::Close);
        assert_eq!(t.hit(1920, 1, (x + 54) as f64, 20.0), BarHit::Minimize);
        assert_eq!(t.hit(1920, 1, (x + 82) as f64, 20.0), BarHit::Restore);
        assert_eq!(t.hit(1920, 1, (x + w - 26) as f64, 20.0), BarHit::Pin);
        assert_eq!(t.hit(1920, 1, (x + w / 2) as f64, 5.0), BarHit::Bar);
        assert_eq!(t.hit(1920, 1, (x + w / 2) as f64, (h + 3) as f64), BarHit::None);
        assert_eq!(t.hit(1920, 1, 3.0, 5.0), BarHit::None);
    }

    #[test]
    fn toolbar_session_tools_hit() {
        let mut t = Toolbar::new("host".into());
        let (x, _, w, _) = t.rect(1920, 1);
        let last = x + w - 26 - 32;
        assert_eq!(t.hit(1920, 1, last as f64, 20.0), BarHit::Pause);
        assert_eq!(t.hit(1920, 1, (last - 32) as f64, 20.0), BarHit::Scale);
        assert_eq!(t.hit(1920, 1, (last - 64) as f64, 20.0), BarHit::Shot);
        assert_eq!(t.hit(1920, 1, (last - 96) as f64, 20.0), BarHit::Cad);
        // docked header: tools move to the far right, no pin
        t.docked = true;
        let last = 1920 - 22;
        assert_eq!(t.hit(1920, 1, last as f64, 20.0), BarHit::Pause);
        assert_eq!(t.hit(1920, 1, (last - 96) as f64, 20.0), BarHit::Cad);
        assert_ne!(t.hit(1920, 1, 1920.0 - 5.0, 20.0), BarHit::Pin);
        // hover text follows state
        assert_eq!(t.tip(BarHit::Pause), Some("Pause"));
        t.paused = true;
        assert_eq!(t.tip(BarHit::Pause), Some("Resume"));
        assert_eq!(t.tip(BarHit::Bar), None);
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
        let mut t2 = Toolbar::new("x".into());
        t2.paused = true;
        t2.actual = true;
        for hit in [BarHit::Cad, BarHit::Shot, BarHit::Scale, BarHit::Pause, BarHit::Pin] {
            t2.draw(&mut c, 2, hit);
            t2.docked = !t2.docked;
        }
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
        let mut buf = vec![0x00_30_60_a0u32; w * h];
        {
            let mut c = Canvas { buf: &mut buf, w, h };
            let mut t = Toolbar::new("nexdesk - 172.31.203.37".into());
            t.docked = true;
            t.draw(&mut c, 1, BarHit::Close);
        }
        save("docked.ppm", &buf);
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
