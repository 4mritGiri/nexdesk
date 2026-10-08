//! The viewer's small toolbar and toast, drawn by software on top of the remote picture.
//! Layout and hit testing are plain functions so they can be tested without a window.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Switch between "fit to window" and 1:1.
    Scale,
    Fullscreen,
    /// Ctrl+Alt+Del on the remote computer.
    Cad,
    Screenshot,
    /// Show the next monitor of the remote computer.
    Monitor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    pub action: Action,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

const BTN_H: i32 = 24;
const PAD: i32 = 8;
const GAP: i32 = 4;
const TOP: i32 = 6;
pub const HOT_EDGE: i32 = 8;

fn text_w(s: &str) -> i32 {
    s.chars().count() as i32 * 8
}

/// The bar rectangle (x, y, w, h) and its buttons, centred at the top of a window `win_w` wide.
pub fn layout(win_w: u32, labels: &[(Action, String)]) -> ((i32, i32, i32, i32), Vec<Item>) {
    let widths: Vec<i32> = labels.iter().map(|(_, l)| text_w(l) + 20).collect();
    let inner: i32 = widths.iter().sum::<i32>() + GAP * (labels.len() as i32 - 1).max(0);
    let bar_w = inner + 2 * PAD;
    let bar_x = ((win_w as i32 - bar_w) / 2).max(0);
    let mut x = bar_x + PAD;
    let mut items = Vec::new();
    for ((action, _), w) in labels.iter().zip(widths) {
        items.push(Item { action: *action, x, y: TOP + 5, w, h: BTN_H });
        x += w + GAP;
    }
    ((bar_x, TOP, bar_w, BTN_H + 10), items)
}

pub fn hit(items: &[Item], x: f64, y: f64) -> Option<Action> {
    let (x, y) = (x.floor() as i32, y.floor() as i32);
    items.iter().find(|i| x >= i.x && x < i.x + i.w && y >= i.y && y < i.y + i.h).map(|i| i.action)
}

pub fn in_bar(bar: (i32, i32, i32, i32), x: f64, y: f64) -> bool {
    let (x, y) = (x.floor() as i32, y.floor() as i32);
    x >= bar.0 && x < bar.0 + bar.2 && y >= bar.1 && y < bar.1 + bar.3
}

fn rect(buf: &mut [u32], w: usize, h: usize, x: i32, y: i32, rw: i32, rh: i32, color: u32) {
    let x0 = (x.max(0) as usize).min(w);
    let y0 = (y.max(0) as usize).min(h);
    let x1 = ((x + rw).max(0) as usize).min(w);
    let y1 = ((y + rh).max(0) as usize).min(h);
    for yy in y0..y1 {
        buf[yy * w + x0..yy * w + x1.max(x0)].fill(color);
    }
}

fn text(buf: &mut [u32], w: usize, h: usize, x: i32, y: i32, s: &str, color: u32) {
    let mut cx = x;
    for ch in s.chars() {
        let idx = if (ch as u32) < 128 { ch as usize } else { b'?' as usize };
        let glyph = font8x8::legacy::BASIC_LEGACY[idx];
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..8 {
                if bits >> col & 1 == 1 {
                    rect(buf, w, h, cx + col, y + row as i32, 1, 1, color);
                }
            }
        }
        cx += 8;
    }
}

const BG: u32 = 0x00_1b_1e_2b;
const BORDER: u32 = 0x00_3a_3f_55;
const BTN: u32 = 0x00_26_2a_3d;
const BTN_HOVER: u32 = 0x00_3b_42_66;
const FG: u32 = 0x00_e6_e8_f0;

/// Draw the bar and its buttons.
pub fn draw_bar(buf: &mut [u32], w: usize, h: usize, bar: (i32, i32, i32, i32), items: &[Item], labels: &[(Action, String)], hover: Option<Action>) {
    rect(buf, w, h, bar.0 - 1, bar.1 - 1, bar.2 + 2, bar.3 + 2, BORDER);
    rect(buf, w, h, bar.0, bar.1, bar.2, bar.3, BG);
    for (item, (_, label)) in items.iter().zip(labels) {
        let bg = if hover == Some(item.action) { BTN_HOVER } else { BTN };
        rect(buf, w, h, item.x, item.y, item.w, item.h, bg);
        text(buf, w, h, item.x + 10, item.y + (item.h - 8) / 2, label, FG);
    }
}

/// A one-line message at the bottom centre.
pub fn draw_toast(buf: &mut [u32], w: usize, h: usize, msg: &str) {
    let msg: String = msg.chars().filter(|c| !c.is_control()).take(((w as i32 - 40) / 8).max(1) as usize).collect();
    let tw = text_w(&msg) + 24;
    let x = (w as i32 - tw) / 2;
    let y = h as i32 - 48;
    rect(buf, w, h, x - 1, y - 1, tw + 2, 34, BORDER);
    rect(buf, w, h, x, y, tw, 32, BG);
    text(buf, w, h, x + 12, y + 12, &msg, FG);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> Vec<(Action, String)> {
        vec![
            (Action::Scale, "1:1".into()),
            (Action::Fullscreen, "Full".into()),
            (Action::Cad, "Ctrl+Alt+Del".into()),
            (Action::Screenshot, "Shot".into()),
        ]
    }

    #[test]
    fn bar_is_centred_and_buttons_do_not_overlap() {
        let (bar, items) = layout(1000, &labels());
        assert_eq!(items.len(), 4);
        let left = bar.0;
        let right = 1000 - (bar.0 + bar.2);
        assert!((left - right).abs() <= 1, "centred: {left} vs {right}");
        for w in items.windows(2) {
            assert!(w[0].x + w[0].w <= w[1].x, "{w:?}");
        }
        assert!(items.iter().all(|i| i.x >= bar.0 && i.x + i.w <= bar.0 + bar.2));
    }

    #[test]
    fn hit_testing() {
        let (bar, items) = layout(1000, &labels());
        let cad = items[2];
        assert_eq!(hit(&items, f64::from(cad.x + 3), f64::from(cad.y + 3)), Some(Action::Cad));
        assert_eq!(hit(&items, f64::from(cad.x - 1), f64::from(cad.y + 3)).is_some(), false, "the gap between buttons is not a button");
        assert!(in_bar(bar, f64::from(bar.0 + 1), f64::from(bar.1 + 1)));
        assert!(!in_bar(bar, 5.0, 500.0));
    }

    #[test]
    fn narrow_window_never_draws_outside_the_buffer() {
        let (bar, items) = layout(50, &labels());
        let mut buf = vec![0u32; 50 * 40];
        draw_bar(&mut buf, 50, 40, bar, &items, &labels(), Some(Action::Scale));
        draw_toast(&mut buf, 50, 40, "a very long message that does not fit into this tiny window at all");
    }
}
