//! Vector icons for the viewer overlay. Each icon is the `d` data of an SVG path on a 24x24 grid, drawn
//! with a round-capped stroke (the same style as the manager's icons in `nexdesk-ui/assets/icons`).
//! The parser understands M L H V C A Z (absolute and relative); curves are flattened to short segments and
//! every pixel is shaded from its distance to the nearest segment, so edges are anti-aliased at any size.
use super::gfx::Canvas;

pub const CAMERA: &str =
    "M4 8h3l1.5-2h7L17 8h3v11H4z M15.5 13.5a3.5 3.5 0 1 1-7 0 3.5 3.5 0 0 1 7 0";
pub const KEYBOARD: &str = "M3 6h18v12H3z M7 10h.01 M11 10h.01 M15 10h.01 M18 10h.01 M7 14h10";
pub const EXPAND: &str = "M4 9V4h5 M20 9V4h-5 M4 15v5h5 M20 15v5h-5";
pub const SHRINK: &str = "M9 4v5H4 M15 4v5h5 M9 20v-5H4 M15 20v-5h5";
pub const CHAT: &str = "M4 5h16v11H11l-4 4v-4H4z M8 9.5h8 M8 12.5h5";
pub const CURSOR: &str = "M5 3l14 7-6 2.2L10.8 18z";
pub const SLASH: &str = "M4 4l16 16";
pub const PIN: &str = "M9 4h6 M10 4v6l-3 5h10l-3-5V4 M12 15v6";
pub const SEND: &str = "M4 12L20 4l-6 16-2.5-6.5z M11.5 13.5L20 4";
pub const CLOSE: &str = "M6 6l12 12 M18 6L6 18";
pub const MONITOR: &str = "M3 5h18v11H3z M9 20h6 M12 16v4";

type Pt = (f32, f32);

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn skip(&mut self) {
        while self.i < self.s.len() && (self.s[self.i] == b' ' || self.s[self.i] == b',') {
            self.i += 1;
        }
    }
    fn cmd(&mut self) -> Option<u8> {
        self.skip();
        let c = *self.s.get(self.i)?;
        if c.is_ascii_alphabetic() {
            self.i += 1;
            Some(c)
        } else {
            None
        }
    }
    fn more(&mut self) -> bool {
        self.skip();
        self.s
            .get(self.i)
            .map(|c| !c.is_ascii_alphabetic())
            .unwrap_or(false)
    }
    fn num(&mut self) -> Option<f32> {
        self.skip();
        let st = self.i;
        let mut seen_dot = false;
        if matches!(self.s.get(self.i), Some(b'-' | b'+')) {
            self.i += 1;
        }
        while let Some(&c) = self.s.get(self.i) {
            if c.is_ascii_digit() {
                self.i += 1;
            } else if c == b'.' && !seen_dot {
                seen_dot = true;
                self.i += 1;
            } else {
                break;
            }
        }
        std::str::from_utf8(&self.s[st..self.i]).ok()?.parse().ok()
    }
    /// An arc flag is a single 0 or 1, possibly written without a separator ("a3.5 3.5 0 1 1-7 0").
    fn flag(&mut self) -> Option<bool> {
        self.skip();
        let c = *self.s.get(self.i)?;
        self.i += 1;
        Some(c == b'1')
    }
}

fn arc(out: &mut Vec<Pt>, from: Pt, rx: f32, ry: f32, large: bool, sweep: bool, to: Pt) {
    // SVG endpoint arc -> centre form (circles only: rx == ry is all the icons need)
    let r = rx.max(ry).max(0.001);
    let (dx, dy) = ((from.0 - to.0) / 2.0, (from.1 - to.1) / 2.0);
    let d2 = dx * dx + dy * dy;
    if d2 == 0.0 {
        return;
    }
    let r = r.max(d2.sqrt());
    let h = ((r * r - d2) / d2).max(0.0).sqrt();
    let sign = if large == sweep { -1.0 } else { 1.0 };
    let (mx, my) = ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0);
    let (cx, cy) = (mx + sign * h * dy, my - sign * h * dx);
    let a0 = (from.1 - cy).atan2(from.0 - cx);
    let a1 = (to.1 - cy).atan2(to.0 - cx);
    let mut da = a1 - a0;
    if sweep && da < 0.0 {
        da += std::f32::consts::TAU;
    }
    if !sweep && da > 0.0 {
        da -= std::f32::consts::TAU;
    }
    let n = ((da.abs() * r) / 0.4).ceil().max(2.0) as usize;
    for k in 1..=n {
        let a = a0 + da * k as f32 / n as f32;
        out.push((cx + r * a.cos(), cy + r * a.sin()));
    }
}

/// Flatten path data into polylines (one per subpath).
fn flatten(d: &str) -> Vec<Vec<Pt>> {
    let mut p = Parser {
        s: d.as_bytes(),
        i: 0,
    };
    let mut subs: Vec<Vec<Pt>> = Vec::new();
    let (mut cur, mut start) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32));
    while let Some(c) = p.cmd() {
        let rel = c.is_ascii_lowercase();
        let up = c.to_ascii_uppercase();
        let off = |pt: Pt, base: Pt| {
            if rel {
                (pt.0 + base.0, pt.1 + base.1)
            } else {
                pt
            }
        };
        if up == b'Z' {
            if let Some(s) = subs.last_mut() {
                s.push(start);
            }
            cur = start;
            continue;
        }
        let mut first = true;
        while first || p.more() {
            first = false;
            match up {
                b'M' => {
                    let (Some(x), Some(y)) = (p.num(), p.num()) else {
                        return subs;
                    };
                    cur = off((x, y), cur);
                    start = cur;
                    subs.push(vec![cur]);
                }
                b'L' => {
                    let (Some(x), Some(y)) = (p.num(), p.num()) else {
                        return subs;
                    };
                    cur = off((x, y), cur);
                    if let Some(s) = subs.last_mut() {
                        s.push(cur);
                    }
                }
                b'H' => {
                    let Some(x) = p.num() else { return subs };
                    cur.0 = if rel { cur.0 + x } else { x };
                    if let Some(s) = subs.last_mut() {
                        s.push(cur);
                    }
                }
                b'V' => {
                    let Some(y) = p.num() else { return subs };
                    cur.1 = if rel { cur.1 + y } else { y };
                    if let Some(s) = subs.last_mut() {
                        s.push(cur);
                    }
                }
                b'C' => {
                    let mut q = [(0.0, 0.0); 3];
                    for slot in &mut q {
                        let (Some(x), Some(y)) = (p.num(), p.num()) else {
                            return subs;
                        };
                        *slot = off((x, y), cur);
                    }
                    let p0 = cur;
                    if let Some(s) = subs.last_mut() {
                        for k in 1..=12 {
                            let t = k as f32 / 12.0;
                            let u = 1.0 - t;
                            s.push((
                                u * u * u * p0.0
                                    + 3.0 * u * u * t * q[0].0
                                    + 3.0 * u * t * t * q[1].0
                                    + t * t * t * q[2].0,
                                u * u * u * p0.1
                                    + 3.0 * u * u * t * q[0].1
                                    + 3.0 * u * t * t * q[1].1
                                    + t * t * t * q[2].1,
                            ));
                        }
                    }
                    cur = q[2];
                }
                b'A' => {
                    let (Some(rx), Some(ry), Some(_rot)) = (p.num(), p.num(), p.num()) else {
                        return subs;
                    };
                    let (Some(large), Some(sweep)) = (p.flag(), p.flag()) else {
                        return subs;
                    };
                    let (Some(x), Some(y)) = (p.num(), p.num()) else {
                        return subs;
                    };
                    let to = off((x, y), cur);
                    if let Some(s) = subs.last_mut() {
                        arc(s, cur, rx, ry, large, sweep, to);
                    }
                    cur = to;
                }
                _ => return subs,
            }
        }
    }
    subs
}

fn seg_dist(p: Pt, a: Pt, b: Pt) -> f32 {
    let (abx, aby) = (b.0 - a.0, b.1 - a.1);
    let len2 = abx * abx + aby * aby;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p.0 - a.0) * abx + (p.1 - a.1) * aby) / len2).clamp(0.0, 1.0)
    };
    let (qx, qy) = (a.0 + abx * t, a.1 + aby * t);
    ((p.0 - qx).powi(2) + (p.1 - qy).powi(2)).sqrt()
}

/// Draw `path` centred on (cx, cy), `size` pixels wide for the 24 unit grid. `stroke` is in grid units
/// (2.0 is the usual weight).
pub fn draw(c: &mut Canvas, path: &str, cx: i32, cy: i32, size: i32, stroke: f32, color: u32) {
    let k = size as f32 / 24.0;
    let (ox, oy) = (cx as f32 - size as f32 / 2.0, cy as f32 - size as f32 / 2.0);
    let segs: Vec<(Pt, Pt)> = flatten(path)
        .iter()
        .flat_map(|poly| {
            poly.windows(2)
                .map(|w| {
                    (
                        (ox + w[0].0 * k, oy + w[0].1 * k),
                        (ox + w[1].0 * k, oy + w[1].1 * k),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let half = stroke * k / 2.0;
    let pad = half.ceil() as i32 + 1;
    for y in (oy as i32 - pad)..(oy as i32 + size + pad) {
        for x in (ox as i32 - pad)..(ox as i32 + size + pad) {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            let d = segs
                .iter()
                .map(|(a, b)| seg_dist(p, *a, *b))
                .fold(f32::MAX, f32::min);
            let cover = (half + 0.5 - d).clamp(0.0, 1.0);
            if cover > 0.0 {
                c.blend(x, y, color, cover);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_parses_and_draws_inside_its_box() {
        for (n, d) in [
            CAMERA, KEYBOARD, EXPAND, SHRINK, CHAT, CURSOR, SLASH, PIN, SEND, CLOSE, MONITOR,
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                flatten(d).iter().any(|p| p.len() >= 2),
                "icon {n} has no strokes"
            );
            let (w, h) = (40usize, 40usize);
            let mut buf = vec![0u32; w * h];
            let mut c = Canvas::new(&mut buf, w, h);
            draw(&mut c, d, 20, 20, 24, 2.0, 0x00ff_ffff);
            let lit = buf.iter().filter(|p| **p != 0).count();
            assert!(lit > 20, "icon {n} drew {lit} pixels");
            // nothing outside the 24 px box plus its stroke
            for y in 0..h {
                for x in 0..w {
                    if !(6..34).contains(&x) || !(6..34).contains(&y) {
                        assert_eq!(buf[y * w + x], 0, "icon {n} bleeds at {x},{y}");
                    }
                }
            }
        }
    }

    #[test]
    fn circle_arc_closes_on_itself() {
        let polys = flatten("M15.5 13.5a3.5 3.5 0 1 1-7 0 3.5 3.5 0 0 1 7 0");
        let all: Vec<Pt> = polys.concat();
        // all points lie 3.5 from (12, 13.5)
        for (x, y) in all.iter().skip(1) {
            let r = ((x - 12.0).powi(2) + (y - 13.5).powi(2)).sqrt();
            assert!((r - 3.5).abs() < 0.2, "{x},{y} r={r}");
        }
    }
}
