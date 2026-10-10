//! Palettes: `Midnight` (deep navy, default), `Graphite` (neutral GNOME Files grey) and `Light`.
//! The active theme is process-wide so Preferences can switch it live.
use std::sync::atomic::{AtomicU8, Ordering};

use gpui::{linear_color_stop, linear_gradient, rgb, rgba, Background, Rgba};
use nexdesk_core::settings::Theme;

static THEME: AtomicU8 = AtomicU8::new(0);

pub fn set_theme(t: Theme) {
    THEME.store(
        match t {
            Theme::Midnight => 0,
            Theme::Graphite => 1,
            Theme::Light => 2,
        },
        Ordering::Relaxed,
    );
}

fn idx() -> u8 {
    THEME.load(Ordering::Relaxed)
}

fn pick(m: u32, g: u32, l: u32) -> Rgba {
    rgb(match idx() {
        1 => g,
        2 => l,
        _ => m,
    })
}

/// Window background: a soft diagonal gradient in Midnight, flat in the others.
pub fn window_bg() -> Background {
    let (a, b) = match idx() {
        1 => (0x242424, 0x242424),
        2 => (0xf4f5fa, 0xf4f5fa),
        _ => (0x0e1019, 0x1a1d33),
    };
    linear_gradient(
        155.,
        linear_color_stop(rgb(a), 0.),
        linear_color_stop(rgb(b), 1.),
    )
}

pub fn bg() -> Rgba {
    pick(0x11131d, 0x242424, 0xf4f5fa)
}
pub fn panel() -> Rgba {
    pick(0x1b1f31, 0x303030, 0xffffff)
}
pub fn panel_2() -> Rgba {
    pick(0x272c45, 0x3d3d3d, 0xe6e9f3)
}
pub fn hover() -> Rgba {
    pick(0x343a5c, 0x484848, 0xd6dbee)
}
pub fn sidebar() -> Rgba {
    pick(0x141725, 0x2b2b2b, 0xeaecf5)
}
pub fn input_bg() -> Rgba {
    pick(0x0d0f18, 0x262626, 0xffffff)
}
pub fn text() -> Rgba {
    pick(0xf1f3fb, 0xffffff, 0x1b1e30)
}
pub fn muted() -> Rgba {
    pick(0xa9b2cf, 0x9a9996, 0x555b73)
}
pub fn dim() -> Rgba {
    pick(0x6a7295, 0x626262, 0x9aa0b8)
}
pub fn icon() -> Rgba {
    pick(0xd3daf2, 0xdedede, 0x3d4358)
}
pub fn accent() -> Rgba {
    pick(0x4c8dff, 0x3584e4, 0x2f6df0)
}
pub fn accent_hover() -> Rgba {
    pick(0x78a9ff, 0x4a93ec, 0x1f56cc)
}
pub fn icon_bg() -> Rgba {
    pick(0x1a2c55, 0x1b3350, 0xdbe6ff)
}
pub fn popover() -> Rgba {
    pick(0x20243a, 0x383838, 0xffffff)
}
pub fn border() -> Rgba {
    pick(0x2c3254, 0x3f3f3f, 0xd3d8e8)
}
pub fn danger() -> Rgba {
    pick(0xff7a8a, 0xff7b63, 0xd4304a)
}
pub fn warn() -> Rgba {
    pick(0xf5c451, 0xf5c211, 0xa86a00)
}
pub fn ok() -> Rgba {
    pick(0x34d27b, 0x28c840, 0x16894a)
}
pub fn blue_dark() -> Rgba {
    icon_bg()
}
pub fn on_accent() -> Rgba {
    rgb(0xffffff)
}
pub fn row_hover() -> Rgba {
    if idx() == 2 {
        rgba(0x00000010)
    } else {
        rgba(0xffffff14)
    }
}
pub fn accent_soft() -> Rgba {
    match idx() {
        1 => rgba(0x3584e44d),
        2 => rgba(0x2f6df02e),
        _ => rgba(0x4c8dff40),
    }
}
pub fn scrim() -> Rgba {
    if idx() == 2 {
        rgba(0x00000066)
    } else {
        rgba(0x000000aa)
    }
}
// Window-control dots (same in every theme, like macOS).
pub fn dot_close() -> Rgba {
    rgb(0xff5f57)
}
pub fn dot_min() -> Rgba {
    rgb(0xfebc2e)
}
pub fn dot_max() -> Rgba {
    rgb(0x28c840)
}
