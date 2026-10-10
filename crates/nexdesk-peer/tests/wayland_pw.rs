//! PipeWire receiving half of the Wayland host, against a real PipeWire daemon and a test video source.
//! Needs `NEXDESK_PW_NODE=<node id>` (see docs/PEER.md); passes without it.
#![cfg(feature = "wayland")]
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

#[test]
fn receives_frames_from_a_pipewire_node() {
    let Some(node) = std::env::var("NEXDESK_PW_NODE")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
    else {
        eprintln!("NEXDESK_PW_NODE not set, skipping");
        return;
    };
    let dir = std::env::var("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
    let sock = UnixStream::connect(format!("{dir}/pipewire-0")).expect("pipewire socket");
    let fd: OwnedFd = sock.into();
    let wl = nexdesk_peer::wayland::Wl::attach_for_test(fd, node).expect("attach");
    let (w, h) = wl
        .wait_first_frame(Duration::from_secs(10))
        .expect("no frame arrived");
    assert!(w >= 16 && h >= 16, "{w}x{h}");
    let (gw, gh, data) = wl.grab().expect("grab");
    assert_eq!((gw, gh), (w, h));
    assert_eq!(data.len(), w as usize * h as usize * 4);
    // videotestsrc draws colour bars: the picture must not be a single flat colour
    let first = &data[0..4];
    assert!(data.chunks_exact(4).any(|p| p != first), "picture is flat");
    // frames keep coming
    let s1 = wl.state().unwrap().2;
    std::thread::sleep(Duration::from_millis(500));
    assert!(wl.state().unwrap().2 > s1, "no new frames");
    drop(wl);
}
