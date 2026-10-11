//! Writes `overlay-preview.ppm`: the viewer overlay drawn over a dummy desktop, to look at the design without a display.
use nexdesk_peer::overlay::{self, gfx::Canvas, Action, BarItem, BarView, ChatView, Mode};
fn main() {
    let (w, h) = (1280usize, 720usize);
    let mut buf: Vec<u32> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            0x00203a5a + (((x * 40 / w) as u32) << 8) + ((y * 60 / h) as u32)
        })
        .collect();
    let it = |action, label: &str, on, group| BarItem {
        action,
        label: label.to_string(),
        on,
        group,
    };
    let items = vec![
        it(Action::Scale, "Actual size (1:1)", false, 0),
        it(Action::Cad, "Send Ctrl+Alt+Del", false, 1),
        it(Action::Screenshot, "Copy screen to clipboard", false, 2),
        it(Action::Control, "Pause control (Ctrl+Alt+G)", true, 1),
        it(Action::Chat, "Chat (Ctrl+Alt+C)", true, 2),
    ];
    let mut c = Canvas::new(&mut buf, w, h);
    let bar = overlay::bar_layout(w as u32, 1, true, &items);
    let view = BarView {
        title: "nexdesk - 495585340",
        dot: overlay::dot_live(),
        fullscreen: false,
        pinned: false,
    };
    overlay::draw_bar(&mut c, &bar, &items, &view, Some(Action::Cad));
    let g = overlay::chat_layout(w as u32, h as u32, 40);
    let msgs = vec![
        (false, "Hi, can you see my screen?".to_string()),
        (
            true,
            "Yes, it is clear. Opening the report now.".to_string(),
        ),
        (false, "Great, thanks for helping".to_string()),
    ];
    overlay::draw_chat(
        &mut c,
        &g,
        &ChatView {
            msgs: &msgs,
            input: "typing a reply",
            focused: true,
            scroll: 0,
        },
    );
    overlay::draw_toast(&mut c, "Copied the screen to the clipboard");
    overlay::draw_chip(&mut c, Mode::Controlling, "");
    let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
    for p in &buf {
        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    std::fs::write("overlay-preview.ppm", out).unwrap();
}
