//! Small software drawing layer for the viewer overlay: anti-aliased rounded rectangles and text.
//! Pixels are `0x00RRGGBB` (what softbuffer expects). Everything is clipped to the buffer.
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

static FONT_BYTES: &[u8] = include_bytes!("../../assets/fonts/Inter-Medium.otf");

fn font() -> &'static fontdue::Font {
    static F: OnceLock<fontdue::Font> = OnceLock::new();
    F.get_or_init(|| {
        fontdue::Font::from_bytes(FONT_BYTES, fontdue::FontSettings::default())
            .expect("the bundled font is valid")
    })
}

type Glyph = (fontdue::Metrics, Vec<u8>);

fn glyph(ch: char, size: f32) -> std::sync::Arc<Glyph> {
    static CACHE: OnceLock<Mutex<HashMap<(char, u32), std::sync::Arc<Glyph>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (ch, (size * 10.0) as u32);
    let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
    if c.len() > 2000 {
        c.clear();
    }
    c.entry(key)
        .or_insert_with(|| std::sync::Arc::new(font().rasterize(ch, size)))
        .clone()
}

/// Characters the bundled font has no glyph for are shown as '?'.
fn shown(ch: char) -> char {
    if ch.is_control() {
        ' '
    } else if font().lookup_glyph_index(ch) == 0 {
        '?'
    } else {
        ch
    }
}

/// Width in pixels of `s` at `size` px.
pub fn text_width(s: &str, size: f32) -> i32 {
    s.chars()
        .map(|c| font().metrics(shown(c), size).advance_width)
        .sum::<f32>()
        .ceil() as i32
}

/// Line height in pixels for `size`.
pub fn line_height(size: f32) -> i32 {
    (size * 1.4).round() as i32
}

/// Split `s` into lines no wider than `max_w`, breaking at spaces and, for long words, anywhere.
pub fn wrap(s: &str, size: f32, max_w: i32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in s.split(' ') {
        let candidate = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if text_width(&candidate, size) <= max_w {
            cur = candidate;
            continue;
        }
        if !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
        }
        // the word alone may still be too wide
        for ch in word.chars() {
            let mut t = cur.clone();
            t.push(ch);
            if text_width(&t, size) > max_w && !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            cur.push(ch);
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Shorten `s` with an ellipsis so it fits `max_w`.
pub fn fit(s: &str, size: f32, max_w: i32) -> String {
    if text_width(s, size) <= max_w {
        return s.to_string();
    }
    let mut out = String::new();
    for ch in s.chars() {
        let mut t = out.clone();
        t.push(ch);
        t.push('\u{2026}');
        if text_width(&t, size) > max_w {
            break;
        }
        out.push(ch);
    }
    out.push('\u{2026}');
    out
}

fn mix(dst: u32, src: u32, a: f32) -> u32 {
    let a = a.clamp(0.0, 1.0);
    let ch = |shift: u32| {
        let d = ((dst >> shift) & 0xff) as f32;
        let s = ((src >> shift) & 0xff) as f32;
        (d + (s - d) * a).round() as u32
    };
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

pub struct Canvas<'a> {
    pub buf: &'a mut [u32],
    pub w: usize,
    pub h: usize,
}

impl<'a> Canvas<'a> {
    pub fn new(buf: &'a mut [u32], w: usize, h: usize) -> Self {
        debug_assert!(buf.len() >= w * h);
        Self { buf, w, h }
    }

    fn put(&mut self, x: i32, y: i32, color: u32, alpha: f32) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h || alpha <= 0.0 {
            return;
        }
        let i = y as usize * self.w + x as usize;
        if let Some(px) = self.buf.get_mut(i) {
            *px = mix(*px, color, alpha);
        }
    }

    /// Rounded rectangle with a soft edge. `alpha` 1.0 is opaque.
    pub fn rrect(&mut self, x: i32, y: i32, w: i32, h: i32, r: f32, color: u32, alpha: f32) {
        if w <= 0 || h <= 0 {
            return;
        }
        let r = r.min(w as f32 / 2.0).min(h as f32 / 2.0).max(0.0);
        let (x0, x1) = (x.max(0), (x + w).min(self.w as i32));
        let (y0, y1) = (y.max(0), (y + h).min(self.h as i32));
        for py in y0..y1 {
            for px in x0..x1 {
                // distance from the pixel centre to the rounded shape's edge (negative inside)
                let cx = px as f32 + 0.5 - (x as f32 + w as f32 / 2.0);
                let cy = py as f32 + 0.5 - (y as f32 + h as f32 / 2.0);
                let qx = cx.abs() - (w as f32 / 2.0 - r);
                let qy = cy.abs() - (h as f32 / 2.0 - r);
                let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0)
                    - r;
                let cover = (0.5 - d).clamp(0.0, 1.0);
                self.put(px, py, color, cover * alpha);
            }
        }
    }

    /// Rounded rectangle with a 1 px border.
    pub fn panel(&mut self, x: i32, y: i32, w: i32, h: i32, r: f32, fill: u32, border: u32, alpha: f32) {
        self.rrect(x, y, w, h, r, border, alpha);
        self.rrect(x + 1, y + 1, w - 2, h - 2, (r - 1.0).max(0.0), fill, alpha);
    }

    /// A soft drop shadow under a rounded rectangle.
    pub fn shadow(&mut self, x: i32, y: i32, w: i32, h: i32, r: f32) {
        for i in 1..=6 {
            let g = i * 2;
            self.rrect(x - g + 2, y - g + 6, w + 2 * g - 4, h + 2 * g - 4, r + g as f32, 0x00_00_00_00, 0.035);
        }
    }

    pub fn hline(&mut self, x: i32, y: i32, w: i32, color: u32, alpha: f32) {
        for px in x..x + w {
            self.put(px, y, color, alpha);
        }
    }

    pub fn vline(&mut self, x: i32, y: i32, h: i32, color: u32, alpha: f32) {
        for py in y..y + h {
            self.put(x, py, color, alpha);
        }
    }

    pub fn disc(&mut self, cx: i32, cy: i32, r: i32, color: u32) {
        self.rrect(cx - r, cy - r, 2 * r, 2 * r, r as f32, color, 1.0);
    }

    /// Draw `s` with its text box's top-left at (x, y). Returns the width drawn.
    pub fn text(&mut self, x: i32, y: i32, s: &str, size: f32, color: u32) -> i32 {
        let ascent = font()
            .horizontal_line_metrics(size)
            .map(|m| m.ascent)
            .unwrap_or(size * 0.9);
        let baseline = y as f32 + (line_height(size) as f32 - size * 1.2) / 2.0 + ascent;
        let mut pen = x as f32;
        for ch in s.chars() {
            let ch = shown(ch);
            let g = glyph(ch, size);
            let (m, bitmap) = (&g.0, &g.1);
            let gx = pen.round() as i32 + m.xmin;
            let gy = baseline.round() as i32 - m.height as i32 - m.ymin;
            for row in 0..m.height {
                for col in 0..m.width {
                    let c = bitmap[row * m.width + col];
                    if c > 0 {
                        self.put(gx + col as i32, gy + row as i32, color, f32::from(c) / 255.0);
                    }
                }
            }
            pen += m.advance_width;
        }
        (pen - x as f32).ceil() as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_draws_pixels_and_measures() {
        let (w, h) = (120, 30);
        let mut buf = vec![0u32; w * h];
        let mut c = Canvas::new(&mut buf, w, h);
        let drawn = c.text(4, 4, "Chat", 14.0, 0x00ff_ffff);
        assert!(drawn > 10 && drawn == text_width("Chat", 14.0));
        assert!(buf.iter().any(|&p| p != 0), "no glyph pixels drawn");
    }

    #[test]
    fn rounded_rect_is_clipped_and_soft() {
        let (w, h) = (20, 20);
        let mut buf = vec![0u32; w * h];
        let mut c = Canvas::new(&mut buf, w, h);
        c.rrect(-5, -5, 40, 40, 8.0, 0x00ff_ffff, 1.0); // bigger than the buffer: must not panic
        assert_eq!(buf[10 * w + 10], 0x00ff_ffff);
        let mut buf2 = vec![0u32; w * h];
        Canvas::new(&mut buf2, w, h).rrect(0, 0, 20, 20, 10.0, 0x00ff_ffff, 1.0);
        assert!(buf2[0] < 0x0020_2020, "the corner stays transparent");
    }

    #[test]
    fn wrapping_respects_width() {
        let lines = wrap("a quick brown fox jumps over the lazy dog", 13.0, 90);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| text_width(l, 13.0) <= 90));
        let long = wrap("supercalifragilisticexpialidocious", 13.0, 60);
        assert!(long.len() > 1 && long.iter().all(|l| text_width(l, 13.0) <= 60));
        assert_eq!(wrap("", 13.0, 60), vec![String::new()]);
    }

    #[test]
    fn fit_adds_an_ellipsis() {
        assert_eq!(fit("short", 13.0, 200), "short");
        let f = fit("a much longer piece of text than fits", 13.0, 60);
        assert!(f.ends_with('\u{2026}') && text_width(&f, 13.0) <= 60);
    }
}
