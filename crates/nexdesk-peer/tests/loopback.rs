//! End-to-end on a real (virtual) X server. Run with: xvfb-run -a cargo test -p nexdesk-peer --test loopback
//! Skipped (passes) when no DISPLAY is available.
use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nexdesk_crypto::Identity;
use nexdesk_peer::host::{run, Policy};
use nexdesk_peer::screen::Screen;
use nexdesk_peer::{client, Msg};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt as _};

/// Both tests share one X server, and one of them resizes it.
static X_SERVER: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn have_display() -> bool {
    std::env::var_os("DISPLAY").is_some()
}

fn start_agent(policy: Policy) -> (String, nexdesk_crypto::IdentityPublic) {
    let id = Identity::generate().unwrap();
    let public = id.public().clone();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    std::thread::spawn(move || run(l, Arc::new(id), Arc::new(policy), |_| {}));
    (addr, public)
}

fn pointer(conn: &impl Connection, root: u32) -> (i16, i16) {
    let r = conn.query_pointer(root).unwrap().reply().unwrap();
    (r.root_x, r.root_y)
}

fn wait_for(mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    false
}

#[test]
fn view_control_and_policy_on_a_real_x_server() {
    if !have_display() {
        eprintln!("no DISPLAY, skipping");
        return;
    }
    let _x = X_SERVER.lock().unwrap_or_else(|e| e.into_inner());
    let (xc, n) = x11rb::connect(None).unwrap();
    let root = xc.setup().roots[n].root;

    let viewer = Identity::generate().unwrap();
    let viewer_fp = viewer.public().fingerprint_string();

    // ---- allowed viewer: sees the screen, controls the pointer
    xc.change_window_attributes(root, &ChangeWindowAttributesAux::new().background_pixel(0x00_20_a0_40)).unwrap();
    xc.clear_area(false, root, 0, 0, 0, 0).unwrap();
    xc.flush().unwrap();
    let (addr, agent_pub) = start_agent(Policy::new(false, true, vec![viewer_fp.clone()], None));
    let pinned = agent_pub.fingerprint();
    let (mut reader, mut writer, seen) = client::connect(&addr, viewer, |p| p.fingerprint() == pinned).unwrap();
    assert_eq!(seen, agent_pub);

    let Msg::Hello { view_only, width, height } = reader.recv().unwrap() else { panic!("expected Hello") };
    assert!(!view_only);
    let mut screen = Screen::new(width, height);
    let mut covered = 0usize;
    let mut cursor_seen = false;
    while covered < width as usize * height as usize {
        let m = reader.recv().unwrap();
        if let Msg::Tile { w, h, .. } = &m {
            covered += *w as usize * *h as usize;
        }
        cursor_seen |= matches!(m, Msg::Cursor { .. });
        screen.apply(&m).unwrap();
    }
    if !cursor_seen {
        // the cursor may follow the first tiles
        loop {
            match reader.recv().unwrap() {
                Msg::Cursor { .. } => break,
                m => screen.apply(&m).unwrap(),
            }
        }
    }
    assert_eq!(screen.buf[10 * width as usize + 10], 0x00_20_a0_40, "root colour must arrive intact");

    writer.send(&Msg::MouseMove { x: 123, y: 77 }).unwrap();
    assert!(wait_for(|| pointer(&xc, root) == (123, 77)), "pointer did not move: {:?}", pointer(&xc, root));
    writer.send(&Msg::Ping(7)).unwrap();
    loop {
        if reader.recv().unwrap() == Msg::Pong(7) {
            break;
        }
    }
    // ---- clipboard: viewer -> agent -> X clipboard, and back
    {
        use nexdesk_clipboard::transport::{Provider, Reply, Transport, TransportEvent};
        use nexdesk_clipboard::X11Transport;
        use std::sync::mpsc::channel;
        struct T(Vec<u8>);
        impl Provider for T {
            fn request(&self, t: &str, r: Reply) {
                r((t == "UTF8_STRING").then(|| self.0.clone()))
            }
        }
        let (etx, erx) = channel();
        let etx = std::sync::Mutex::new(etx);
        let x = X11Transport::spawn(Arc::new(move |e| {
            let _ = etx.lock().unwrap().send(e);
        }))
        .unwrap();
        writer.send(&Msg::Clip("grüße from the viewer".into())).unwrap();
        let got = (0..100).find_map(|_| {
            x.read(9, "UTF8_STRING");
            let end = Instant::now() + Duration::from_millis(200);
            while Instant::now() < end {
                if let Ok(TransportEvent::ReadDone { token: 9, result: Ok(b) }) = erx.recv_timeout(Duration::from_millis(50)) {
                    if b == "grüße from the viewer".as_bytes() {
                        return Some(b);
                    }
                }
            }
            None
        });
        assert!(got.is_some(), "viewer clipboard never reached the agent's X clipboard");
        x.own(vec!["UTF8_STRING".into()], Arc::new(T("copied on the host".as_bytes().to_vec())));
        let t = Instant::now();
        let mut seen_clip = false;
        while !seen_clip {
            assert!(t.elapsed() < Duration::from_secs(5), "host clipboard never reached the viewer");
            match reader.recv().unwrap() {
                Msg::Clip(c) if c == "copied on the host" => seen_clip = true,
                m => {
                    screen.apply(&m).unwrap();
                }
            }
        }
    }

    // a later change of the screen arrives as new tiles
    xc.change_window_attributes(root, &ChangeWindowAttributesAux::new().background_pixel(0x00_c0_10_10)).unwrap();
    xc.clear_area(false, root, 0, 0, 0, 0).unwrap();
    xc.flush().unwrap();
    let t = Instant::now();
    while screen.buf[10 * width as usize + 10] != 0x00_c0_10_10 {
        assert!(t.elapsed() < Duration::from_secs(5), "update never arrived");
        let m = reader.recv().unwrap();
        screen.apply(&m).unwrap();
    }
    writer.shutdown();

    // ---- unknown viewer without a prompt is refused (the viewer finds out on its first read)
    let (addr2, pub2) = start_agent(Policy::new(false, true, vec![], None));
    let p2 = pub2.fingerprint();
    let (mut r2, _w2, _) = client::connect(&addr2, Identity::generate().unwrap(), |p| p.fingerprint() == p2).unwrap();
    assert!(r2.recv().is_err(), "an unapproved viewer must not receive a screen");

    // ---- wrong pin: the viewer refuses before revealing itself
    let (addr3, _) = start_agent(Policy::new(false, true, vec![], None));
    let r = client::connect(&addr3, Identity::generate().unwrap(), |_| false);
    assert!(matches!(r, Err(nexdesk_peer::PeerError::Crypto(nexdesk_crypto::Error::Rejected))));

    // ---- view-only: pointer must not move
    let v = Identity::generate().unwrap();
    let vfp = v.public().fingerprint_string();
    let (addr4, _) = start_agent(Policy::new(true, true, vec![vfp], None));
    let (mut r4, mut w4, _) = client::connect(&addr4, v, |_| true).unwrap();
    let Msg::Hello { view_only, .. } = r4.recv().unwrap() else { panic!() };
    assert!(view_only);
    let before = pointer(&xc, root);
    w4.send(&Msg::MouseMove { x: 300, y: 200 }).unwrap();
    w4.send(&Msg::Ping(1)).unwrap();
    while r4.recv().unwrap() != Msg::Pong(1) {}
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(pointer(&xc, root), before, "view-only viewers cannot control the pointer");
    w4.shutdown();
}

/// Remove the monitors the monitor test defines, so tests do not depend on their order.
fn remove_test_monitors(xc: &impl Connection, root: u32) {
    use x11rb::protocol::randr::ConnectionExt as _;
    for name in ["nd-left", "nd-right"] {
        if let Ok(a) = xc.intern_atom(true, name.as_bytes()).and_then(|c| Ok(c.reply())) {
            if let Ok(a) = a {
                if a.atom != 0 {
                    let _ = xc.randr_delete_monitor(root, a.atom).map(|c| c.check());
                }
            }
        }
    }
}

/// Changing the X screen size must not end the session: the agent sends a new Hello.
#[test]
fn screen_resize_is_announced_not_fatal() {
    if !have_display() {
        return;
    }
    let _x = X_SERVER.lock().unwrap_or_else(|e| e.into_inner());
    let (xc, n) = x11rb::connect(None).unwrap();
    let root = xc.setup().roots[n].root;
    remove_test_monitors(&xc, root);
    let v = Identity::generate().unwrap();
    let (addr, agent_pub) = start_agent(Policy::new(false, true, vec![v.public().fingerprint_string()], None));
    let pinned = agent_pub.fingerprint();
    let (mut reader, _writer, _) = client::connect(&addr, v, |p| p.fingerprint() == pinned).unwrap();
    let Msg::Hello { width, .. } = reader.recv().unwrap() else { panic!() };
    let (nw, nh) = if width == 1024 { (800u16, 600u16) } else { (1024, 768) };
    {
        use x11rb::protocol::randr::ConnectionExt as _;
        // the screen may only shrink below the active mode once the CRTC is switched off
        let res = xc.randr_get_screen_resources(root).unwrap().reply().unwrap();
        for crtc in res.crtcs {
            let info = xc.randr_get_crtc_info(crtc, 0).unwrap().reply().unwrap();
            if info.mode != 0 {
                xc.randr_set_crtc_config(crtc, 0, res.config_timestamp, 0, 0, 0u32, x11rb::protocol::randr::Rotation::ROTATE0, &[])
                    .unwrap()
                    .reply()
                    .unwrap();
            }
        }
    }
    x11rb::protocol::randr::set_screen_size(&xc, root, nw, nh, 300, 200).unwrap().check().expect("set_screen_size");
    xc.flush().unwrap();
    let t = Instant::now();
    loop {
        assert!(t.elapsed() < Duration::from_secs(8), "no new Hello after the resize");
        match reader.recv().unwrap() {
            Msg::Hello { width: w2, height: h2, .. } => {
                assert_eq!((w2, h2), (nw, nh));
                break;
            }
            Msg::Bye(why) => panic!("session ended on resize: {why}"),
            _ => {}
        }
    }
}

fn next_msg(reader: &mut nexdesk_peer::Reader, want: &mut dyn FnMut(&Msg) -> bool) -> Msg {
    let t = Instant::now();
    loop {
        assert!(t.elapsed() < Duration::from_secs(8), "expected message never came");
        let m = reader.recv().unwrap();
        if want(&m) {
            return m;
        }
    }
}

/// Two monitors: the viewer is told about both, can switch, and the mouse lands on the chosen one.
#[test]
fn monitor_selection_and_idle_screen() {
    if !have_display() {
        return;
    }
    use x11rb::protocol::randr::{self, ConnectionExt as _, MonitorInfo};
    let _x = X_SERVER.lock().unwrap_or_else(|e| e.into_inner());
    let (xc, n) = x11rb::connect(None).unwrap();
    let root = xc.setup().roots[n].root;
    let res = xc.randr_get_screen_resources(root).unwrap().reply().unwrap();
    for crtc in res.crtcs {
        let info = xc.randr_get_crtc_info(crtc, 0).unwrap().reply().unwrap();
        if info.mode != 0 {
            xc.randr_set_crtc_config(crtc, 0, res.config_timestamp, 0, 0, 0u32, randr::Rotation::ROTATE0, &[]).unwrap().reply().unwrap();
        }
    }
    randr::set_screen_size(&xc, root, 1280, 600, 340, 160).unwrap().check().expect("set_screen_size");
    for (name, x) in [("nd-left", 0i16), ("nd-right", 640i16)] {
        let atom = xc.intern_atom(false, name.as_bytes()).unwrap().reply().unwrap().atom;
        let m = MonitorInfo { name: atom, primary: x == 0, automatic: false, x, y: 0, width: 640, height: 600, width_in_millimeters: 170, height_in_millimeters: 160, outputs: vec![] };
        xc.randr_set_monitor(root, m).unwrap().check().expect("set_monitor");
    }
    xc.flush().unwrap();

    let v = Identity::generate().unwrap();
    let (addr, agent_pub) = start_agent(Policy::new(false, true, vec![v.public().fingerprint_string()], None));
    let pinned = agent_pub.fingerprint();
    let (mut reader, mut writer, _) = client::connect(&addr, v, |p| p.fingerprint() == pinned).unwrap();
    let Msg::Hello { width, height, .. } = next_msg(&mut reader, &mut |m| matches!(m, Msg::Hello { .. })) else { unreachable!() };
    assert_eq!((width, height), (640, 600), "the first monitor, not the whole screen");
    let Msg::Monitors { rects, current } = next_msg(&mut reader, &mut |m| matches!(m, Msg::Monitors { .. })) else { unreachable!() };
    assert_eq!(rects.len(), 2, "{rects:?}");
    assert_eq!(current, 0);

    // idle screen: after the first full picture no more tiles arrive (XDamage reports nothing)
    let mut quiet_rounds = 0;
    for round in 0..40u64 {
        writer.send(&Msg::Ping(1000 + round)).unwrap();
        let mut tiles = 0;
        loop {
            match reader.recv().unwrap() {
                Msg::Pong(n) if n == 1000 + round => break,
                Msg::Tile { .. } => tiles += 1,
                _ => {}
            }
        }
        quiet_rounds = if tiles == 0 { quiet_rounds + 1 } else { 0 };
        if quiet_rounds >= 3 {
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    assert!(quiet_rounds >= 3, "an idle screen must stop producing tiles");

    writer.send(&Msg::SelectMonitor(1)).unwrap();
    let Msg::Hello { width, height, .. } = next_msg(&mut reader, &mut |m| matches!(m, Msg::Hello { .. })) else { unreachable!() };
    assert_eq!((width, height), (640, 600));
    let Msg::Monitors { current, rects } = next_msg(&mut reader, &mut |m| matches!(m, Msg::Monitors { .. })) else { unreachable!() };
    assert_eq!(current, 1);
    let second = rects[1];
    writer.send(&Msg::MouseMove { x: 10, y: 20 }).unwrap();
    assert!(wait_for(|| pointer(&xc, root) == (second.x + 10, second.y + 20)), "pointer must land on the chosen monitor: {:?} vs {second:?}", pointer(&xc, root));
    // an out of range index is clamped, not an error
    writer.send(&Msg::SelectMonitor(200)).unwrap();
    writer.send(&Msg::Ping(9)).unwrap();
    next_msg(&mut reader, &mut |m| *m == Msg::Pong(9));
    writer.shutdown();
    remove_test_monitors(&xc, root);
}
