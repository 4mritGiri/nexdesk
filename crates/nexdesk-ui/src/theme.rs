//! Palette modelled on libadwaita's dark style (GNOME Files / Nautilus).
use gpui::{rgb, rgba, Rgba};

/// Window / content background.
pub fn bg() -> Rgba {
    rgb(0x242424)
}
/// Cards, dialogs, header bar.
pub fn panel() -> Rgba {
    rgb(0x303030)
}
/// Buttons, selected rows.
pub fn panel_2() -> Rgba {
    rgb(0x3d3d3d)
}
pub fn hover() -> Rgba {
    rgb(0x484848)
}
pub fn row_hover() -> Rgba {
    rgba(0xffffff12)
}
pub fn sidebar() -> Rgba {
    rgb(0x2b2b2b)
}
pub fn input_bg() -> Rgba {
    rgb(0x262626)
}
pub fn text() -> Rgba {
    rgb(0xffffff)
}
pub fn muted() -> Rgba {
    rgb(0x9a9996)
}
/// Libadwaita blue.
pub fn accent() -> Rgba {
    rgb(0x3584e4)
}
pub fn accent_hover() -> Rgba {
    rgb(0x4a93ec)
}
pub fn accent_soft() -> Rgba {
    rgba(0x3584e44d)
}
pub fn on_accent() -> Rgba {
    rgb(0xffffff)
}
pub fn border() -> Rgba {
    rgb(0x3f3f3f)
}
pub fn danger() -> Rgba {
    rgb(0xff7b63)
}
pub fn scrim() -> Rgba {
    rgba(0x00000099)
}
// Window-control dots (same hues as the reference screenshot).
pub fn dot_close() -> Rgba {
    rgb(0xff5f57)
}
pub fn dot_min() -> Rgba {
    rgb(0xfebc2e)
}
pub fn dot_max() -> Rgba {
    rgb(0x28c840)
}
