//! The screen-share portal path on a real Wayland compositor (view-only). Needs a Wayland session with
//! xdg-desktop-portal and `NEXDESK_PORTAL_TEST=<r>,<g>,<b>`: the colour the desktop background is set to.
#![cfg(feature = "wayland")]
use std::time::Duration;

#[test]
fn shares_a_wayland_screen_through_the_portal() {
    let Some(rgb) = std::env::var("NEXDESK_PORTAL_TEST").ok() else {
        eprintln!("NEXDESK_PORTAL_TEST not set, skipping");
        return;
    };
    let want: Vec<u8> = rgb.split(',').map(|v| v.trim().parse().unwrap()).collect();
    let wl = nexdesk_peer::wayland::Wl::start(false).expect("portal share");
    let (w, h) = wl
        .wait_first_frame(Duration::from_secs(15))
        .expect("no picture");
    eprintln!("got {w}x{h}");
    assert!(w >= 320 && h >= 200);
    let (_, _, data) = wl.grab().expect("grab");
    let px = &data[(h as usize / 2 * w as usize + w as usize / 2) * 4..][..4];
    // BGRX
    assert_eq!(
        [px[2], px[1], px[0]],
        [want[0], want[1], want[2]],
        "background colour must arrive intact, got {px:?}"
    );
    drop(wl); // closes the portal session
}
