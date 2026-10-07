//! `nexdesk-peer-view HOST:PORT`: window that shows a remote agent's screen and forwards input.
use std::io::BufRead;
use std::num::NonZeroU32;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use nexdesk_core::knownhosts::{host_key, KnownHosts, Lookup};
use nexdesk_core::scale::{blit_fit, Fit};
use nexdesk_peer::screen::Screen;
use nexdesk_peer::{client, store, Msg, Writer};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::platform::scancode::PhysicalKeyExtScancode;
use winit::window::{CustomCursor, Window, WindowId};
use nexdesk_peer::clip::ClipSync;
use nexdesk_peer::wire::unpack_pixels;

enum Ev {
    Msg(Msg),
    Status(String),
    Gone(String),
}

struct App {
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    screen: Option<Screen>,
    out: mpsc::Sender<Msg>,
    view_only: bool,
    title: String,
    cursor: (f64, f64),
    clipboard: bool,
    clip: Option<ClipSync>,
}

impl App {
    fn fit(&self) -> Option<Fit> {
        let s = self.window.as_ref()?.inner_size();
        let sc = self.screen.as_ref()?;
        Fit::new(sc.w, sc.h, s.width, s.height)
    }

    fn draw(&mut self) {
        let (Some(win), Some(surface)) = (self.window.as_ref(), self.surface.as_mut()) else { return };
        let size = win.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        if surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buf) = surface.buffer_mut() else { return };
        match &self.screen {
            Some(sc) => blit_fit(&sc.buf, sc.w, sc.h, &mut buf, size.width, size.height),
            None => buf.fill(0x00_11_13_1d),
        }
        let _ = buf.present();
    }
}

impl ApplicationHandler<Ev> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title(self.title.clone()).with_inner_size(LogicalSize::new(1280, 720));
        let Ok(win) = el.create_window(attrs) else {
            el.exit();
            return;
        };
        let win = Arc::new(win);
        let Ok(ctx) = softbuffer::Context::new(win.clone()) else { return el.exit() };
        let Ok(surface) = softbuffer::Surface::new(&ctx, win.clone()) else { return el.exit() };
        self.surface = Some(surface);
        self.window = Some(win);
    }

    fn user_event(&mut self, el: &ActiveEventLoop, ev: Ev) {
        match ev {
            Ev::Msg(Msg::Hello { view_only, width, height }) => {
                self.view_only = view_only;
                if self.clipboard && !view_only && self.clip.is_none() {
                    match ClipSync::start(self.out.clone()) {
                        Ok(c) => self.clip = Some(c),
                        Err(e) => eprintln!("clipboard sharing unavailable: {e}"),
                    }
                }
                self.screen = Some(Screen::new(width, height));
                if let Some(w) = &self.window {
                    w.set_title(&format!("{}{}", self.title, if view_only { " (view only)" } else { "" }));
                }
            }
            Ev::Msg(m @ Msg::Tile { .. }) => {
                if let Some(s) = self.screen.as_mut() {
                    if s.apply(&m).is_err() {
                        eprintln!("bad tile from the agent, closing");
                        el.exit();
                    }
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            Ev::Msg(Msg::Clip(t)) => {
                if let Some(c) = &self.clip {
                    c.set_remote(t);
                }
            }
            Ev::Msg(Msg::Cursor { hot_x, hot_y, w, h, lz4 }) => {
                if let (Some(win), Ok(px)) = (&self.window, unpack_pixels(&lz4, w, h)) {
                    let rgba = nexdesk_peer::screen::cursor_rgba(&px);
                    if let Ok(src) = CustomCursor::from_rgba(rgba, w, h, hot_x, hot_y) {
                        win.set_cursor(el.create_custom_cursor(src));
                    }
                }
            }
            Ev::Status(s) => {
                if let Some(w) = &self.window {
                    w.set_title(&format!("{} ({s})", self.title));
                }
            }
            Ev::Msg(Msg::Bye(why)) => {
                eprintln!("agent closed the session: {why}");
                el.exit();
            }
            Ev::Msg(_) => {}
            Ev::Gone(why) => {
                eprintln!("disconnected: {why}");
                el.exit();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::Resized(_) => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ if self.view_only => {}
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                if let Some(f) = self.fit() {
                    let (x, y) = f.to_remote(position.x, position.y);
                    let _ = self.out.send(Msg::MouseMove { x, y });
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let b = match button {
                    MouseButton::Left => 1,
                    MouseButton::Middle => 2,
                    MouseButton::Right => 3,
                    _ => return,
                };
                let _ = self.out.send(Msg::MouseButton { button: b, down: state == ElementState::Pressed });
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * 120.0, y * 120.0),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                let _ = self.out.send(Msg::Wheel { dx: dx.clamp(-32768.0, 32767.0) as i16, dy: dy.clamp(-32768.0, 32767.0) as i16 });
            }
            WindowEvent::KeyboardInput { event, .. } if !event.repeat => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    // winit's raw scancode on Linux is the X11 keycode = evdev code + 8
                    if let Some(sc) = PhysicalKey::Code(code).to_scancode() {
                        if sc >= 8 {
                            let _ = self.out.send(Msg::Key { code: (sc - 8) as u16, down: event.state == ElementState::Pressed });
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Keep the session alive: forward messages, send pings, and after a network drop reconnect to the
/// same pinned agent (backing off, for about 90 s). A connection that ended before the agent sent its
/// Hello (denied, refused) or with a Bye is final: asking the agent's user again and again would be abuse.
fn supervise(
    first: (nexdesk_peer::Reader, Writer),
    rx: mpsc::Receiver<Msg>,
    proxy: winit::event_loop::EventLoopProxy<Ev>,
    addr: String,
    agent_fp: String,
    dir: std::path::PathBuf,
) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let mut conn = Some(first);
    let (mut delay, mut waited) = (1u64, 0u64);
    let mut reconnected = false;
    loop {
        let (mut reader, mut writer) = match conn.take() {
            Some(c) => c,
            None => {
                let attempt = store::load_or_create_identity(&dir, "viewer-identity")
                    .map_err(|e| e.to_string())
                    .and_then(|me| {
                        client::connect(&addr, me, |a| a.fingerprint_string() == agent_fp).map_err(|e| e.to_string())
                    });
                match attempt {
                    Ok((r, w, _)) => {
                        while rx.try_recv().is_ok() {} // never replay stale keys or clicks
                        (delay, waited, reconnected) = (1, 0, true);
                        (r, w)
                    }
                    Err(e) => {
                        if waited >= 90 {
                            let _ = proxy.send_event(Ev::Gone(format!("could not reconnect: {e}")));
                            return;
                        }
                        let _ = proxy.send_event(Ev::Status(format!("reconnecting, retry in {delay} s")));
                        std::thread::sleep(Duration::from_secs(delay));
                        waited += delay;
                        delay = (delay * 2).min(8);
                        continue;
                    }
                }
            }
        };
        let ended = Arc::new(AtomicBool::new(false));
        let established = Arc::new(AtomicBool::new(false));
        let said_bye = Arc::new(AtomicBool::new(false));
        let reader_thread = {
            let (ended, established, said_bye, proxy) = (ended.clone(), established.clone(), said_bye.clone(), proxy.clone());
            std::thread::spawn(move || {
                loop {
                    match reader.recv() {
                        Ok(m) => {
                            match &m {
                                Msg::Hello { .. } => established.store(true, Ordering::SeqCst),
                                Msg::Bye(_) => said_bye.store(true, Ordering::SeqCst),
                                _ => {}
                            }
                            if proxy.send_event(Ev::Msg(m)).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                ended.store(true, Ordering::SeqCst);
            })
        };
        let mut n = 0u64;
        let mut last_ping = std::time::Instant::now();
        let mut window_closed = false;
        while !ended.load(Ordering::SeqCst) {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(m) => {
                    if writer.send(&m).is_err() {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    window_closed = true;
                    break;
                }
            }
            if last_ping.elapsed() >= Duration::from_secs(10) {
                last_ping = std::time::Instant::now();
                n += 1;
                if writer.send(&Msg::Ping(n)).is_err() {
                    break;
                }
            }
        }
        writer.shutdown();
        let _ = reader_thread.join();
        if window_closed || said_bye.load(Ordering::SeqCst) {
            return;
        }
        if !established.load(Ordering::SeqCst) {
            let why = if reconnected { "the agent did not accept the reconnection" } else { "the agent closed the connection" };
            let _ = proxy.send_event(Ev::Gone(why.into()));
            return;
        }
        let _ = proxy.send_event(Ev::Status("connection lost, reconnecting".into()));
    }
}

fn ask(question: &str) -> bool {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return false; // no terminal, no way to ask: refuse (use --trust or --probe)
    }
    eprint!("{question} [y/N] ");
    let mut s = String::new();
    let _ = std::io::stdin().lock().read_line(&mut s);
    s.trim().eq_ignore_ascii_case("y")
}

fn die(m: &str) -> ! {
    eprintln!("{m}");
    std::process::exit(2);
}

fn main() {
    let mut forget = false;
    let mut addr = None;
    let (mut probe, mut clipboard, mut trust) = (false, true, None::<String>);
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--forget" => forget = true,
            "--probe" => probe = true,
            "--no-clipboard" => clipboard = false,
            "--trust" => trust = Some(it.next().unwrap_or_else(|| die("--trust needs a fingerprint"))),
            "-h" | "--help" => {
                println!("nexdesk-peer-view HOST:PORT [--forget] [--no-clipboard] [--trust SHA256:..] [--probe]\n  --forget  drop the pinned identity of this agent first\n  --trust   pin the agent if its fingerprint is exactly this\n  --probe   print \"FINGERPRINT <fp>\" and exit (no session)");
                return;
            }
            _ => addr = Some(a),
        }
    }
    let addr = addr.unwrap_or_else(|| die("usage: nexdesk-peer-view HOST:PORT"));
    let dir = store::default_dir().unwrap_or_else(|| die("HOME is not set"));
    let me = store::load_or_create_identity(&dir, "viewer-identity").unwrap_or_else(|e| die(&e.to_string()));
    let mut known = KnownHosts::load(&dir.join("known_agents"));
    let key = host_key(addr.rsplit_once(':').map(|x| x.0).unwrap_or(&addr), addr.rsplit_once(':').and_then(|x| x.1.parse().ok()).unwrap_or(nexdesk_peer::DEFAULT_PORT));
    if forget {
        let _ = known.forget(&key);
    }

    let mut pin_error = None;
    let result = client::connect(&addr, me, |agent| {
        let fp = agent.fingerprint_string();
        if probe {
            println!("FINGERPRINT {fp}");
            return false;
        }
        match known.lookup(&key, &fp) {
            Lookup::Match => true,
            Lookup::Unknown if trust.as_deref() == Some(fp.as_str()) => {
                let _ = known.set(&key, &fp);
                true
            }
            Lookup::Unknown => {
                eprintln!("First connection to {key}.\nThe agent's fingerprint is\n  {fp}\nCompare it with the one printed on the agent's screen.");
                if ask("Trust this agent?") {
                    let _ = known.set(&key, &fp);
                    true
                } else {
                    false
                }
            }
            Lookup::Mismatch { pinned } => {
                pin_error = Some(format!("THE AGENT'S IDENTITY CHANGED\n  pinned: {pinned}\n  now:    {fp}\nSomeone may be impersonating it. If it was reinstalled, run with --forget."));
                false
            }
        }
    });
    if probe {
        std::process::exit(0);
    }
    let (reader, writer, peer) = match result {
        Ok(x) => x,
        Err(e) => die(&pin_error.unwrap_or_else(|| format!("connection failed: {e}"))),
    };
    eprintln!("connected to {} - waiting for the agent's user to approve if asked", peer.fingerprint_string());

    let addr_for_retry = addr.clone();
    let el = EventLoop::<Ev>::with_user_event().build().unwrap_or_else(|e| die(&e.to_string()));
    let proxy = el.create_proxy();
    let (tx, rx) = mpsc::channel::<Msg>();
    let agent_fp = peer.fingerprint_string();
    std::thread::spawn(move || supervise((reader, writer), rx, proxy, addr_for_retry, agent_fp, dir));
    let mut app = App {
        window: None,
        surface: None,
        screen: None,
        out: tx,
        view_only: false,
        title: format!("nexdesk - {addr}"),
        cursor: (0.0, 0.0),
        clipboard,
        clip: None,
    };
    let _ = el.run_app(&mut app);
}
