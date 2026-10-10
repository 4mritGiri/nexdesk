//! The viewer's toolbar, chat panel, toast and status chip, drawn by software on top of the remote picture.
//! Geometry and hit testing are plain functions (no window needed) so they are unit tested; `gfx` draws.
pub mod gfx;

use gfx::{fit, line_height, text_width, wrap, Canvas};

/// What a toolbar button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Switch between "fit to window" and 1:1.
    Scale,
    Fullscreen,
    /// Show the next monitor of the remote computer.
    Monitor,
    /// Send this computer's mouse and keyboard to the remote one, or stop sending them.
    Control,
    /// Ctrl+Alt+Del on the remote computer.
    Cad,
    Screenshot,
    /// Open or close the chat panel.
    Chat,
}

/// One toolbar button as the application describes it.
#[derive(Debug, Clone)]
pub struct BarItem {
    pub action: Action,
    pub label: String,
    /// Highlighted (for example chat is open, control is on).
    pub on: bool,
    /// Buttons of different groups are separated by a thin line.
    pub group: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        let (x, y) = (x.floor() as i32, y.floor() as i32);
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// What the pointer is over, among the overlay's own controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Bar(Action),
    /// Empty part of the toolbar: it still swallows the click.
    BarBackground,
    ChatClose,
    ChatInput,
    /// Anywhere else on the chat panel.
    ChatPanel,
}

// ---- look ------------------------------------------------------------------------------------
const PANEL: u32 = 0x00_16_18_22;
const PANEL_2: u32 = 0x00_1f_22_30;
const BORDER: u32 = 0x00_34_39_4f;
const TEXT: u32 = 0x00_ee_f0_f8;
const MUTED: u32 = 0x00_9a_a0_b8;
const ACCENT: u32 = 0x00_6d_7c_ff;
const GOOD: u32 = 0x00_3e_cf_8e;
const WARN: u32 = 0x00_ff_b8_4d;
const WHITE: u32 = 0x00_ff_ff_ff;
const FONT: f32 = 13.0;
const SMALL: f32 = 11.5;

/// Pointer within this many pixels of the top edge reveals the toolbar.
pub const HOT_EDGE: i32 = 8;

// ---- toolbar ---------------------------------------------------------------------------------
const BTN_H: i32 = 30;
const BAR_H: i32 = 42;
const BAR_TOP: i32 = 10;
const BTN_PAD: i32 = 14;
const GAP: i32 = 2;
const SEP_W: i32 = 11;

#[derive(Debug, Clone)]
pub struct Bar {
    pub rect: Rect,
    pub buttons: Vec<(Action, Rect)>,
    /// x positions of the separators.
    pub seps: Vec<i32>,
}

/// The bar, centred at the top of a window `win_w` wide (it shrinks its padding, never leaves the window).
pub fn bar_layout(win_w: u32, items: &[BarItem]) -> Bar {
    let widths: Vec<i32> = items
        .iter()
        .map(|i| text_width(&i.label, FONT) + 2 * BTN_PAD)
        .collect();
    let mut total = 12; // side padding
    for (n, w) in widths.iter().enumerate() {
        total += w + GAP;
        if n + 1 < items.len() && items[n + 1].group != items[n].group {
            total += SEP_W;
        }
    }
    let bar_w = total.min(win_w as i32 - 8).max(0);
    let bar_x = ((win_w as i32 - bar_w) / 2).max(0);
    let mut x = bar_x + 6;
    let (mut buttons, mut seps) = (Vec::new(), Vec::new());
    for (n, (item, w)) in items.iter().zip(&widths).enumerate() {
        buttons.push((
            item.action,
            Rect {
                x,
                y: BAR_TOP + (BAR_H - BTN_H) / 2,
                w: *w,
                h: BTN_H,
            },
        ));
        x += w + GAP;
        if n + 1 < items.len() && items[n + 1].group != item.group {
            seps.push(x + SEP_W / 2 - 1);
            x += SEP_W;
        }
    }
    Bar {
        rect: Rect {
            x: bar_x,
            y: BAR_TOP,
            w: bar_w,
            h: BAR_H,
        },
        buttons,
        seps,
    }
}

pub fn draw_bar(c: &mut Canvas, bar: &Bar, items: &[BarItem], hover: Option<Action>) {
    let r = bar.rect;
    c.shadow(r.x, r.y, r.w, r.h, BAR_H as f32 / 2.0);
    c.panel(r.x, r.y, r.w, r.h, BAR_H as f32 / 2.0, PANEL, BORDER, 0.94);
    for x in &bar.seps {
        c.vline(*x, r.y + 12, r.h - 24, BORDER, 1.0);
    }
    for ((action, b), item) in bar.buttons.iter().zip(items) {
        let hovered = hover == Some(*action);
        if item.on {
            c.rrect(b.x, b.y, b.w, b.h, b.h as f32 / 2.0, ACCENT, if hovered { 0.42 } else { 0.30 });
        } else if hovered {
            c.rrect(b.x, b.y, b.w, b.h, b.h as f32 / 2.0, WHITE, 0.10);
        }
        let color = if item.on { WHITE } else { TEXT };
        let tw = text_width(&item.label, FONT);
        c.text(b.x + (b.w - tw) / 2, b.y + (b.h - line_height(FONT)) / 2, &item.label, FONT, color);
    }
}

// ---- chat panel ------------------------------------------------------------------------------
const CHAT_W: i32 = 340;
const HEAD_H: i32 = 44;
const FOOT_H: i32 = 56;
const PANEL_MARGIN: i32 = 12;
const BUBBLE_PAD_X: i32 = 12;
const BUBBLE_PAD_Y: i32 = 7;
const BUBBLE_GAP: i32 = 8;

#[derive(Debug, Clone, Copy)]
pub struct ChatGeom {
    pub panel: Rect,
    pub close: Rect,
    pub list: Rect,
    pub input: Rect,
}

/// The chat panel docked at the right edge, below the toolbar.
pub fn chat_layout(win_w: u32, win_h: u32) -> ChatGeom {
    let w = (win_w as i32 - 2 * PANEL_MARGIN).clamp(0, CHAT_W);
    let x = win_w as i32 - w - PANEL_MARGIN;
    let y = BAR_TOP + BAR_H + 10;
    let h = (win_h as i32 - y - PANEL_MARGIN).max(HEAD_H + FOOT_H + 40);
    ChatGeom {
        panel: Rect { x, y, w, h },
        close: Rect {
            x: x + w - 38,
            y: y + 8,
            w: 28,
            h: 28,
        },
        list: Rect {
            x: x + 4,
            y: y + HEAD_H,
            w: w - 8,
            h: h - HEAD_H - FOOT_H,
        },
        input: Rect {
            x: x + 12,
            y: y + h - FOOT_H + 10,
            w: w - 24,
            h: 36,
        },
    }
}

/// A chat message and how it is laid out.
struct Bubble {
    mine: bool,
    lines: Vec<String>,
    w: i32,
    h: i32,
}

fn bubbles(g: &ChatGeom, msgs: &[(bool, String)]) -> Vec<Bubble> {
    let max_text = ((g.list.w - 2 * 12) as f32 * 0.8) as i32 - 2 * BUBBLE_PAD_X;
    msgs.iter()
        .map(|(mine, t)| {
            let lines = wrap(t, FONT, max_text.max(40));
            let w = lines.iter().map(|l| text_width(l, FONT)).max().unwrap_or(0) + 2 * BUBBLE_PAD_X;
            let h = lines.len() as i32 * line_height(FONT) + 2 * BUBBLE_PAD_Y;
            Bubble { mine: *mine, lines, w, h }
        })
        .collect()
}

/// How far (in pixels) the list can be scrolled up from the newest message.
pub fn chat_max_scroll(g: &ChatGeom, msgs: &[(bool, String)]) -> i32 {
    let total: i32 = bubbles(g, msgs).iter().map(|b| b.h + BUBBLE_GAP).sum::<i32>() + BUBBLE_GAP;
    (total - g.list.h).max(0)
}

pub struct ChatView<'a> {
    pub msgs: &'a [(bool, String)],
    pub input: &'a str,
    pub focused: bool,
    /// Pixels scrolled up from the newest message.
    pub scroll: i32,
}

pub fn draw_chat(c: &mut Canvas, g: &ChatGeom, v: &ChatView) {
    let p = g.panel;
    c.shadow(p.x, p.y, p.w, p.h, 14.0);
    c.panel(p.x, p.y, p.w, p.h, 14.0, PANEL, BORDER, 1.0);

    // messages, newest at the bottom; whatever spills over the header or footer is covered below
    let list = g.list;
    let mut y = list.y + list.h - BUBBLE_GAP + v.scroll.clamp(0, chat_max_scroll(g, v.msgs));
    for b in bubbles(g, v.msgs).iter().rev() {
        y -= b.h;
        if y + b.h > list.y - 40 && y < list.y + list.h + 40 {
            let x = if b.mine { list.x + list.w - b.w - 12 } else { list.x + 12 };
            if b.mine {
                c.rrect(x, y, b.w, b.h, 12.0, ACCENT, 1.0);
            } else {
                c.rrect(x, y, b.w, b.h, 12.0, PANEL_2, 1.0);
            }
            for (n, l) in b.lines.iter().enumerate() {
                c.text(x + BUBBLE_PAD_X, y + BUBBLE_PAD_Y + n as i32 * line_height(FONT), l, FONT, if b.mine { WHITE } else { TEXT });
            }
        }
        y -= BUBBLE_GAP;
    }
    if v.msgs.is_empty() {
        let hint = "Messages to the other computer appear here.";
        for (n, l) in wrap(hint, SMALL, list.w - 40).iter().enumerate() {
            c.text(list.x + 20, list.y + 16 + n as i32 * line_height(SMALL), l, SMALL, MUTED);
        }
    }

    // header and footer cover the overflow
    c.rrect(p.x + 1, p.y + 1, p.w - 2, HEAD_H - 1, 13.0, PANEL, 1.0);
    c.rrect(p.x + 1, p.y + HEAD_H - 14, p.w - 2, 14, 0.0, PANEL, 1.0);
    c.hline(p.x + 1, p.y + HEAD_H, p.w - 2, BORDER, 1.0);
    c.text(p.x + 16, p.y + (HEAD_H - line_height(FONT)) / 2, "Chat", FONT + 1.0, TEXT);
    c.text(p.x + 62, p.y + (HEAD_H - line_height(SMALL)) / 2 + 1, "with the other computer", SMALL, MUTED);
    // close button: a plain cross
    let cl = g.close;
    let (cx, cy) = (cl.x + cl.w / 2, cl.y + cl.h / 2);
    for i in -4..=4 {
        c.rrect(cx + i - 1, cy + i - 1, 2, 2, 1.0, MUTED, 1.0);
        c.rrect(cx + i - 1, cy - i - 1, 2, 2, 1.0, MUTED, 1.0);
    }
    c.rrect(p.x + 1, p.y + p.h - FOOT_H, p.w - 2, FOOT_H - 1, 13.0, PANEL, 1.0);
    c.rrect(p.x + 1, p.y + p.h - FOOT_H, p.w - 2, 14, 0.0, PANEL, 1.0);
    c.hline(p.x + 1, p.y + p.h - FOOT_H, p.w - 2, BORDER, 1.0);

    // input field
    let i = g.input;
    c.panel(i.x, i.y, i.w, i.h, 10.0, PANEL_2, if v.focused { ACCENT } else { BORDER }, 1.0);
    let room = i.w - 24;
    if v.input.is_empty() {
        let hint = if v.focused { "Type a message" } else { "Click here to type a message" };
        c.text(i.x + 12, i.y + (i.h - line_height(FONT)) / 2, hint, FONT, MUTED);
    } else {
        // show the end of the text when it is longer than the field
        let mut start = 0;
        let chars: Vec<char> = v.input.chars().collect();
        while start < chars.len() && text_width(&chars[start..].iter().collect::<String>(), FONT) > room {
            start += 1;
        }
        let shown: String = chars[start..].iter().collect();
        let w = c.text(i.x + 12, i.y + (i.h - line_height(FONT)) / 2, &shown, FONT, TEXT);
        if v.focused {
            c.rrect(i.x + 13 + w, i.y + 9, 2, i.h - 18, 1.0, ACCENT, 1.0);
        }
    }
}

// ---- toast and status chip ---------------------------------------------------------------------
/// A one-line message at the bottom centre.
pub fn draw_toast(c: &mut Canvas, msg: &str) {
    let max = (c.w as i32 - 80).max(40);
    let msg = fit(&msg.chars().filter(|ch| !ch.is_control()).collect::<String>(), FONT, max);
    let tw = text_width(&msg, FONT) + 32;
    let (x, y) = ((c.w as i32 - tw) / 2, c.h as i32 - 64);
    c.shadow(x, y, tw, 38, 19.0);
    c.panel(x, y, tw, 38, 19.0, PANEL, BORDER, 0.96);
    c.text(x + 16, y + (38 - line_height(FONT)) / 2, &msg, FONT, TEXT);
}

/// How the session is being driven, always visible in the corner so it is never a surprise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Controlling,
    /// Nothing is sent; the person can move the pointer freely.
    Paused,
    ViewOnly,
}

pub fn draw_chip(c: &mut Canvas, mode: Mode, note: &str) {
    let (dot, label) = match mode {
        Mode::Controlling => (GOOD, "Controlling"),
        Mode::Paused => (WARN, "Control off"),
        Mode::ViewOnly => (MUTED, "View only"),
    };
    let text = if note.is_empty() { label.to_string() } else { format!("{label} \u{2022} {note}") };
    let text = fit(&text, SMALL, (c.w as i32 / 2).max(60));
    let tw = text_width(&text, SMALL) + 36;
    let (x, y) = (12, c.h as i32 - 12 - 26);
    c.panel(x, y, tw, 26, 13.0, PANEL, BORDER, 0.88);
    c.disc(x + 14, y + 13, 4, dot);
    c.text(x + 26, y + (26 - line_height(SMALL)) / 2, &text, SMALL, TEXT);
}

// ---- hit testing -------------------------------------------------------------------------------
/// What the overlay's own controls are at (x, y), if anything. `bar` is None while the toolbar is hidden.
pub fn hit_test(bar: Option<&Bar>, chat: Option<&ChatGeom>, x: f64, y: f64) -> Option<Hit> {
    if let Some(g) = chat {
        if g.close.contains(x, y) {
            return Some(Hit::ChatClose);
        }
        if g.input.contains(x, y) {
            return Some(Hit::ChatInput);
        }
        if g.panel.contains(x, y) {
            return Some(Hit::ChatPanel);
        }
    }
    if let Some(b) = bar {
        if let Some((a, _)) = b.buttons.iter().find(|(_, r)| r.contains(x, y)) {
            return Some(Hit::Bar(*a));
        }
        if b.rect.contains(x, y) {
            return Some(Hit::BarBackground);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<BarItem> {
        let mk = |action, label: &str, group| BarItem {
            action,
            label: label.into(),
            on: false,
            group,
        };
        vec![
            mk(Action::Scale, "1:1", 0),
            mk(Action::Fullscreen, "Full screen", 0),
            mk(Action::Control, "Control", 1),
            mk(Action::Cad, "Ctrl+Alt+Del", 1),
            mk(Action::Screenshot, "Copy screen", 2),
            mk(Action::Chat, "Chat", 2),
        ]
    }

    #[test]
    fn bar_is_centred_inside_the_window_and_buttons_do_not_overlap() {
        let it = items();
        let bar = bar_layout(1280, &it);
        assert!(bar.rect.x >= 0 && bar.rect.x + bar.rect.w <= 1280);
        assert_eq!(bar.seps.len(), 2);
        let mut last = bar.rect.x;
        for (_, r) in &bar.buttons {
            assert!(r.x >= last && r.x + r.w <= bar.rect.x + bar.rect.w);
            last = r.x + r.w;
        }
    }

    #[test]
    fn hit_testing_finds_buttons_chat_and_background() {
        let it = items();
        let bar = bar_layout(1280, &it);
        let (a, r) = bar.buttons[3];
        assert_eq!(hit_test(Some(&bar), None, f64::from(r.x + 3), f64::from(r.y + 3)), Some(Hit::Bar(a)));
        assert_eq!(hit_test(Some(&bar), None, f64::from(bar.rect.x) + 1.0, f64::from(bar.rect.y) + 1.0), Some(Hit::BarBackground));
        assert_eq!(hit_test(None, None, 5.0, 5.0), None);
        let g = chat_layout(1280, 720);
        assert_eq!(hit_test(None, Some(&g), f64::from(g.input.x + 4), f64::from(g.input.y + 4)), Some(Hit::ChatInput));
        assert_eq!(hit_test(None, Some(&g), f64::from(g.close.x + 4), f64::from(g.close.y + 4)), Some(Hit::ChatClose));
        assert_eq!(hit_test(None, Some(&g), f64::from(g.list.x + 4), f64::from(g.list.y + 4)), Some(Hit::ChatPanel));
        // the middle of the screen belongs to the remote computer
        assert_eq!(hit_test(Some(&bar), Some(&g), 400.0, 400.0), None);
    }

    #[test]
    fn chat_stays_inside_small_windows_and_scrolls() {
        for (w, h) in [(300u32, 200u32), (1920, 1080), (50, 50)] {
            let g = chat_layout(w, h);
            assert!(g.panel.x >= 0 || w < 24, "{w}x{h}");
            let mut buf = vec![0u32; (w * h) as usize];
            let mut c = Canvas::new(&mut buf, w as usize, h as usize);
            let msgs: Vec<(bool, String)> = (0..30).map(|i| (i % 2 == 0, format!("message number {i} with some words to wrap around"))).collect();
            draw_chat(&mut c, &g, &ChatView { msgs: &msgs, input: "typing", focused: true, scroll: 99999 });
            draw_bar(&mut c, &bar_layout(w, &items()), &items(), None);
            draw_toast(&mut c, "hello");
            draw_chip(&mut c, Mode::Paused, "Ctrl+Alt+G to resume");
        }
        let g = chat_layout(1280, 720);
        let few: Vec<(bool, String)> = vec![(true, "hi".into())];
        assert_eq!(chat_max_scroll(&g, &few), 0);
        let many: Vec<(bool, String)> = (0..50).map(|i| (false, format!("line {i}"))).collect();
        assert!(chat_max_scroll(&g, &many) > 0);
    }
}
