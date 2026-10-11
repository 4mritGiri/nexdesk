//! The viewer's header bar, drawn like the RDP viewer's: traffic-light window buttons on the left, the
//! connection as a tab, session tools on the right. In a normal window it is a docked full-width header
//! (the window has no system title bar); in full screen it is a floating pill that hangs from the top edge.
//! Geometry and hit testing are plain functions so they are unit tested; `draw_bar` paints.
use super::gfx::{fit, line_height, text_width, Canvas};
use super::icons;
use super::{
    Action, BarItem, Rect, ACCENT, ACCENT_HI, BAR, BAR_HOVER, BG, DIM, EDGE, FG, GREEN, WARN,
};

const RED: u32 = 0x00_ff_5f_57;
const YELLOW: u32 = 0x00_fe_bc_2e;
const OFF_DOT: u32 = 0x00_55_55_55;
const TAB_BG: u32 = 0x00_3a_3a_3a;

/// Header height at UI scale 1.
pub const HEADER_H: i32 = 40;
/// Pointer within this many pixels of the top edge reveals the floating bar.
pub const HOT_EDGE: i32 = 8;
const TOOL_STEP: i32 = 32;
const LIGHT_STEP: i32 = 28;
const TEXT: f32 = 12.5;

#[derive(Debug, Clone)]
pub struct Bar {
    pub rect: Rect,
    pub docked: bool,
    pub u: i32,
    /// Clickable buttons, window buttons first.
    pub buttons: Vec<(Action, Rect)>,
    /// Where the connection tab is drawn.
    pub tab: Rect,
}

/// What the app tells the bar about the session.
#[derive(Debug, Clone)]
pub struct BarView<'a> {
    pub title: &'a str,
    /// Colour of the status dot in the tab (connecting / live / control off).
    pub dot: u32,
    pub fullscreen: bool,
    pub pinned: bool,
}

pub fn dot_live() -> u32 {
    GREEN
}
pub fn dot_idle() -> u32 {
    OFF_DOT
}
pub fn dot_paused() -> u32 {
    WARN
}

/// Lay the bar out for a window `win_w` wide. `items` are the session tools, left to right.
pub fn bar_layout(win_w: u32, u: i32, docked: bool, items: &[BarItem]) -> Bar {
    let win_w = win_w as i32;
    let h = HEADER_H * u;
    let n = items.len() as i32;
    let tools_w = n * TOOL_STEP * u + if docked { 0 } else { TOOL_STEP * u };
    let lights_w = (26 + 3 * LIGHT_STEP) * u;
    let w = if docked {
        win_w
    } else {
        ((lights_w + 150 * u + tools_w + 26 * u).max(540 * u)).min(win_w)
    };
    let x = (win_w - w) / 2;
    let cy = h / 2;
    let mut buttons = Vec::new();
    let c0 = x + 26 * u;
    for (i, a) in [Action::Close, Action::Minimize, Action::Fullscreen]
        .into_iter()
        .enumerate()
    {
        let cx = c0 + i as i32 * LIGHT_STEP * u;
        buttons.push((
            a,
            Rect {
                x: cx - 14 * u,
                y: 0,
                w: 28 * u,
                h,
            },
        ));
    }
    // tools sit on the right; the pin (floating bar only) is the last one
    let last = if docked {
        x + w - 22 * u
    } else {
        x + w - 26 * u - TOOL_STEP * u
    };
    for (i, it) in items.iter().enumerate() {
        let cx = last - (n - 1 - i as i32) * TOOL_STEP * u;
        buttons.push((
            it.action,
            Rect {
                x: cx - (TOOL_STEP / 2) * u,
                y: 0,
                w: TOOL_STEP * u,
                h,
            },
        ));
    }
    if !docked {
        let cx = x + w - 26 * u;
        buttons.push((
            Action::Pin,
            Rect {
                x: cx - 16 * u,
                y: 0,
                w: 32 * u,
                h,
            },
        ));
    }
    let tab_left = c0 + 2 * LIGHT_STEP * u + 24 * u;
    let tools_left = buttons
        .iter()
        .skip(3)
        .map(|(_, r)| r.x)
        .min()
        .unwrap_or(x + w);
    let room = (tools_left - 10 * u - tab_left).max(0);
    let tw = room.min(260 * u);
    Bar {
        rect: Rect { x, y: 0, w, h },
        docked,
        u,
        buttons,
        tab: Rect {
            x: tab_left,
            y: cy - 14 * u,
            w: tw,
            h: 28 * u,
        },
    }
}

fn tip(action: Action, items: &[BarItem], v: &BarView) -> Option<String> {
    let from_item = || {
        items
            .iter()
            .find(|i| i.action == action)
            .map(|i| i.label.clone())
    };
    Some(match action {
        Action::Close => "Disconnect".into(),
        Action::Minimize => "Minimize".into(),
        Action::Fullscreen => "Toggle full screen".into(),
        Action::Pin => {
            if v.pinned {
                "Unpin bar".into()
            } else {
                "Pin bar".into()
            }
        }
        Action::Monitor => format!("Next screen ({})", from_item()?),
        _ => return from_item(),
    })
}

fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = (((a >> s) & 0xff) as f32, ((b >> s) & 0xff) as f32);
        ((x + (y - x) * t).round() as u32) & 0xff
    };
    ch(16) << 16 | ch(8) << 8 | ch(0)
}

pub fn draw_bar(c: &mut Canvas, bar: &Bar, items: &[BarItem], v: &BarView, hover: Option<Action>) {
    let u = bar.u;
    let r = bar.rect;
    if bar.docked {
        c.fill(r.x, r.y, r.w, r.h, BAR);
        c.fill(r.x, r.y + r.h - u, r.w, u, EDGE);
    } else {
        // hangs from the top edge: the top corners are pushed off the window, only the bottom ones show
        c.rrect(
            r.x - u,
            -20 * u,
            r.w + 2 * u,
            r.h + 20 * u + u,
            16.0 * u as f32,
            EDGE,
            1.0,
        );
        c.rrect(r.x, -20 * u, r.w, r.h + 20 * u, 15.0 * u as f32, BAR, 1.0);
    }
    let cy = r.y + r.h / 2;
    for (action, b) in &bar.buttons {
        let cx = b.x + b.w / 2;
        let hot = hover == Some(*action);
        match action {
            Action::Close | Action::Minimize | Action::Fullscreen => {
                let (col, glyph) = match action {
                    Action::Close => (RED, 0x00_5c_10_0c),
                    Action::Minimize => (YELLOW, 0x00_6b_4b_00),
                    _ => (GREEN, 0x00_0b_55_16),
                };
                c.disc(cx, cy, 7 * u, col);
                if hot {
                    let (g, t) = (3 * u, u.max(1));
                    match action {
                        Action::Close => {
                            c.line(cx - g, cy - g, cx + g, cy + g, t, glyph);
                            c.line(cx - g, cy + g, cx + g, cy - g, t, glyph);
                        }
                        Action::Minimize => c.fill(cx - g, cy - t / 2, 2 * g, t, glyph),
                        _ => c.frame(cx - g, cy - g, 2 * g, 2 * g, t, glyph),
                    }
                }
            }
            Action::Pin => {
                if hot {
                    c.disc(cx, cy, 13 * u, BAR_HOVER);
                }
                let col = if v.pinned {
                    ACCENT_HI
                } else if hot {
                    FG
                } else {
                    DIM
                };
                icons::draw(c, icons::PIN, cx, cy, 16 * u, 1.9, col);
            }
            _ => {
                let item = items.iter().find(|i| i.action == *action);
                let on = item.map(|i| i.on).unwrap_or(false);
                // "control off" is the state worth noticing, like the RDP viewer's paused state
                let lit = if *action == Action::Control { !on } else { on };
                let (bx, by, bs) = (cx - 14 * u, cy - 14 * u, 28 * u);
                if hot {
                    c.rrect(bx, by, bs, bs, 8.0 * u as f32, BAR_HOVER, 1.0);
                } else if lit {
                    c.rrect(bx, by, bs, bs, 8.0 * u as f32, mix(BAR, ACCENT, 0.35), 1.0);
                }
                let col = if lit {
                    ACCENT_HI
                } else if hot {
                    FG
                } else {
                    DIM
                };
                draw_icon(c, *action, item, cx, cy, u, col);
            }
        }
    }
    // the connection, as a tab
    let t = bar.tab;
    if t.w > 40 * u {
        c.rrect(t.x, t.y, t.w, t.h, 9.0 * u as f32, TAB_BG, 1.0);
        c.fill(
            t.x + 10 * u,
            t.y + t.h - 2 * u,
            t.w - 20 * u,
            2 * u,
            if v.dot == WARN { WARN } else { ACCENT_HI },
        );
        c.disc(t.x + 12 * u, cy, 4 * u, v.dot);
        let fs = TEXT * u as f32;
        let label = fit(v.title, fs, t.w - 22 * u - 12 * u);
        c.text(t.x + 22 * u, cy - line_height(fs) / 2, &label, fs, FG);
    }
    // tooltip under the hovered button
    if let Some(a) = hover {
        if let (Some(text), Some((_, b))) =
            (tip(a, items, v), bar.buttons.iter().find(|(x, _)| *x == a))
        {
            let fs = TEXT * u as f32;
            let tw = text_width(&text, fs);
            let (bw, bh) = (tw + 16 * u, 22 * u);
            let cx = b.x + b.w / 2;
            let bx = (cx - bw / 2).clamp(2 * u, (c.w as i32 - bw - 2 * u).max(2 * u));
            let by = r.h + 6 * u;
            c.rrect(
                bx - u,
                by - u,
                bw + 2 * u,
                bh + 2 * u,
                7.0 * u as f32,
                EDGE,
                1.0,
            );
            c.rrect(bx, by, bw, bh, 6.0 * u as f32, BG, 1.0);
            c.text(bx + 8 * u, by + (bh - line_height(fs)) / 2, &text, fs, FG);
        }
    }
}

/// Icons for the session tools: SVG paths from `icons`, drawn with the bar's colours.
fn draw_icon(
    c: &mut Canvas,
    a: Action,
    item: Option<&BarItem>,
    cx: i32,
    cy: i32,
    u: i32,
    col: u32,
) {
    let size = 18 * u;
    let on = item.map(|i| i.on).unwrap_or(false);
    match a {
        Action::Cad => icons::draw(c, icons::KEYBOARD, cx, cy, size, 1.9, col),
        Action::Screenshot => icons::draw(c, icons::CAMERA, cx, cy, size, 1.9, col),
        Action::Scale => icons::draw(
            c,
            if on { icons::SHRINK } else { icons::EXPAND },
            cx,
            cy,
            size,
            2.0,
            col,
        ),
        Action::Control => {
            icons::draw(c, icons::CURSOR, cx, cy, size, 1.9, col);
            if !on {
                // control is off: the pointer is struck through
                icons::draw(c, icons::SLASH, cx, cy, size, 2.2, col);
            }
        }
        Action::Chat => icons::draw(c, icons::CHAT, cx, cy, size, 1.9, col),
        Action::Monitor => {
            icons::draw(c, icons::MONITOR, cx - 6 * u, cy, 16 * u, 1.9, col);
            let label = item.map(|i| i.label.as_str()).unwrap_or("");
            let fs = 10.5 * u as f32;
            c.text(cx + 3 * u, cy - line_height(fs) / 2, label, fs, col);
        }
        _ => {}
    }
}
