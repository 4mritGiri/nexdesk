//! `nexdesk-peer-view HOST:PORT`: window that shows a remote agent's screen and forwards input.
use std::io::BufRead;
use std::num::NonZeroU32;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nexdesk_core::knownhosts::{host_key, KnownHosts, Lookup};
use nexdesk_core::scale::{Actual, Fit, View};
use nexdesk_peer::overlay::{self, Action};
use nexdesk_peer::screen::Screen;
use nexdesk_peer::wire::Rect;
use nexdesk_peer::{client, store, Msg, Writer};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::platform::scancode::PhysicalKeyExtScancode;
use winit::window::{CustomCursor, Fullscreen, Window, WindowId};
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
    /// 1:1 instead of fit-to-window, and the part of the remote screen shown at the top left.
    actual: bool,
    pan: (u32, u32),
    fullscreen: bool,
    /// The toolbar is open because the pointer touched the top edge, or for a moment after a shortcut.
    bar_open: bool,
    bar_until: Option<Instant>,
    hover: Option<Action>,
    bar_click: bool,
    toast: Option<(String, Instant)>,
    monitors: Vec<Rect>,
    current_monitor: usize,
    mods: ModifiersState,
    /// A shortcut key whose press was swallowed; its release is swallowed too.
    swallowed: Option<KeyCode>,
}

const SHOW_FOR: Duration = Duration::from_secs(3);
const EDGE_PAN: i32 = 16;
const PAN_STEP: u32 = 24;

impl App {
    fn win_size(&self) -> Option<(u32, u32)> {
        let s = self.window.as_ref()?.inner_size();
        (s.width > 0 && s.height > 0).then_some((s.width, s.height))
    }

    fn view(&self) -> Option<View> {
        let (w, h) = self.win_size()?;
        let sc = self.screen.as_ref()?;
        if self.actual {
            Actual::new(sc.w, sc.h, w, h, self.pan.0, self.pan.1).map(View::Actual)
        } else {
            Fit::new(sc.w, sc.h, w, h).map(View::Fit)
        }
    }

    fn labels(&self) -> Vec<(Action, String)> {
        let mut l = vec![
            (Action::Scale, if self.actual { "Fit" } else { "1:1" }.to_string()),
            (Action::Fullscreen, if self.fullscreen { "Window" } else { "Full" }.to_string()),
        ];
        if self.monitors.len() > 1 {
            l.push((Action::Monitor, format!("Screen {}/{}", self.current_monitor + 1, self.monitors.len())));
        }
        if !self.view_only {
            l.push((Action::Cad, "Ctrl+Alt+Del".to_string()));
        }
        l.push((Action::Screenshot, "Shot".to_string()));
        l
    }

    fn bar_visible(&self) -> bool {
        self.bar_open || self.bar_until.map(|t| t > Instant::now()).unwrap_or(false)
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now() + SHOW_FOR));
        self.redraw();
    }

    fn redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn draw(&mut self) {
        let view = self.view();
        let labels = self.labels();
        let (visible, hover) = (self.bar_visible(), self.hover);
        let toast = self.toast.as_ref().filter(|(_, t)| *t > Instant::now()).map(|(m, _)| m.clone());
        let (Some(win), Some(surface)) = (self.window.as_ref(), self.surface.as_mut()) else { return };
        let size = win.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        if surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buf) = surface.buffer_mut() else { return };
        match (&self.screen, view) {
            (Some(sc), Some(v)) => v.blit(&sc.buf, &mut buf),
            _ => buf.fill(0x00_11_13_1d),
        }
        let (bw, bh) = (size.width as usize, size.height as usize);
        if visible {
            let (bar, items) = overlay::layout(size.width, &labels);
            overlay::draw_bar(&mut buf, bw, bh, bar, &items, &labels, hover);
        }
        if let Some(m) = toast {
            overlay::draw_toast(&mut buf, bw, bh, &m);
        }
        let _ = buf.present();
    }

    fn do_action(&mut self, action: Action) {
        match action {
            Action::Scale => {
                self.actual = !self.actual;
                self.pan = (0, 0);
            }
            Action::Fullscreen => self.toggle_fullscreen(),
            Action::Cad => self.send_cad(),
            Action::Screenshot => self.screenshot(),
            Action::Monitor => {
                if self.monitors.len() > 1 {
                    let next = (self.current_monitor + 1) % self.monitors.len();
                    let _ = self.out.send(Msg::SelectMonitor(next as u8));
                }
            }
        }
        self.redraw();
    }

    fn toggle_fullscreen(&mut self) {
        self.fullscreen = !self.fullscreen;
        if let Some(w) = &self.window {
            w.set_fullscreen(self.fullscreen.then_some(Fullscreen::Borderless(None)));
        }
        self.bar_until = Some(Instant::now() + SHOW_FOR);
    }

    /// Ctrl+Alt+Del as evdev codes: LEFTCTRL 29, LEFTALT 56, DELETE 111.
    fn send_cad(&mut self) {
        if self.view_only {
            return;
        }
        for (code, down) in [(29, true), (56, true), (111, true), (111, false), (56, false), (29, false)] {
            let _ = self.out.send(Msg::Key { code, down });
        }
        self.say("Sent Ctrl+Alt+Del");
    }

    fn screenshot(&mut self) {
        let Some(sc) = self.screen.as_ref() else { return };
        let msg = match nexdesk_peer::shot::save(&sc.buf, sc.w, sc.h, &self.title) {
            Ok(p) => format!("Saved {}", p.display()),
            Err(e) => format!("Screenshot failed: {e}"),
        };
        self.say(msg);
    }

    fn pointer_moved(&mut self, x: f64, y: f64) {
        self.cursor = (x, y);
        let Some((w, _)) = self.win_size() else { return };
        let labels = self.labels();
        let (bar, items) = overlay::layout(w, &labels);
        if y < f64::from(overlay::HOT_EDGE) {
            self.bar_open = true;
        } else if self.bar_open && !overlay::in_bar(bar, x, y) {
            self.bar_open = false;
        }
        let hover = if self.bar_visible() { overlay::hit(&items, x, y) } else { None };
        if hover != self.hover || self.bar_open {
            self.hover = hover;
            self.redraw();
        }
        // 1:1 view of a bigger screen: pointing at a window edge scrolls
        if self.actual {
            if let (Some(View::Actual(a)), Some((ww, wh))) = (self.view(), self.win_size()) {
                if a.pannable() {
                    let (mut px, mut py) = (a.pan_x, a.pan_y);
                    let (xi, yi) = (x as i32, y as i32);
                    if xi < EDGE_PAN {
                        px = px.saturating_sub(PAN_STEP);
                    } else if xi > ww as i32 - EDGE_PAN {
                        px += PAN_STEP;
                    }
                    if yi < EDGE_PAN && !self.bar_open {
                        py = py.saturating_sub(PAN_STEP);
                    } else if yi > wh as i32 - EDGE_PAN {
                        py += PAN_STEP;
                    }
                    let (mx, my) = a.max_pan();
                    let np = (px.min(mx), py.min(my));
                    if np != (a.pan_x, a.pan_y) {
                        self.pan = np;
                        self.redraw();
                    }
                }
            }
        }
    }

    fn over_bar(&self) -> bool {
        let Some((w, _)) = self.win_size() else { return false };
        let (bar, _) = overlay::layout(w, &self.labels());
        self.bar_visible() && overlay::in_bar(bar, self.cursor.0, self.cursor.1)
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

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        let now = Instant::now();
        let mut next: Option<Instant> = None;
        if let Some(t) = self.bar_until {
            if t <= now {
                self.bar_until = None;
                self.redraw();
            } else {
                next = Some(t);
            }
        }
        if let Some((_, t)) = &self.toast {
            if *t <= now {
                self.toast = None;
                self.redraw();
            } else {
                next = Some(next.map_or(*t, |n| n.min(*t)));
            }
        }
        el.set_control_flow(match next {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
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
                self.pan = (0, 0);
                if let Some(w) = &self.window {
                    w.set_title(&format!("{}{}", self.title, if view_only { " (view only)" } else { "" }));
                }
            }
            Ev::Msg(Msg::Monitors { current, rects }) => {
                self.current_monitor = usize::from(current).min(rects.len().saturating_sub(1));
                self.monitors = rects;
                self.redraw();
            }
            Ev::Msg(m @ Msg::Tile { .. }) => {
                if let Some(s) = self.screen.as_mut() {
                    if s.apply(&m).is_err() {
                        eprintln!("bad tile from the agent, closing");
                        el.exit();
                    }
                }
                self.redraw();
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
            WindowEvent::Resized(_) => self.redraw(),
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer_moved(position.x, position.y);
                if self.over_bar() || self.view_only {
                    return;
                }
                if let Some(v) = self.view() {
                    let (x, y) = v.to_remote(position.x, position.y);
                    let _ = self.out.send(Msg::MouseMove { x, y });
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                if button == MouseButton::Left && self.over_bar() && pressed {
                    self.bar_click = true;
                    if let Some(a) = self.hover {
                        self.do_action(a);
                    }
                    return;
                }
                if !pressed && self.bar_click {
                    self.bar_click = false;
                    return;
                }
                if self.view_only {
                    return;
                }
                let b = match button {
                    MouseButton::Left => 1,
                    MouseButton::Middle => 2,
                    MouseButton::Right => 3,
                    _ => return,
                };
                let _ = self.out.send(Msg::MouseButton { button: b, down: pressed });
            }
            WindowEvent::MouseWheel { delta, .. } if !self.view_only && !self.over_bar() => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * 120.0, y * 120.0),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                let _ = self.out.send(Msg::Wheel { dx: dx.clamp(-32768.0, 32767.0) as i16, dy: dy.clamp(-32768.0, 32767.0) as i16 });
            }
            WindowEvent::KeyboardInput { event, .. } if !event.repeat => {
                let PhysicalKey::Code(code) = event.physical_key else { return };
                let down = event.state == ElementState::Pressed;
                // viewer shortcuts: Ctrl+Alt+Pause = full screen, Ctrl+Alt+End = Ctrl+Alt+Del on the remote side
                if !down && self.swallowed == Some(code) {
                    self.swallowed = None;
                    return;
                }
                if down && self.mods.control_key() && self.mods.alt_key() {
                    match code {
                        KeyCode::Pause => {
                            self.swallowed = Some(code);
                            self.toggle_fullscreen();
                            self.redraw();
                            return;
                        }
                        KeyCode::End => {
                            self.swallowed = Some(code);
                            self.send_cad();
                            return;
                        }
                        _ => {}
                    }
                }
                if self.view_only {
                    return;
                }
                // winit's raw scancode on Linux is the X11 keycode = evdev code + 8
                if let Some(sc) = PhysicalKey::Code(code).to_scancode() {
                    if sc >= 8 {
                        let _ = self.out.send(Msg::Key { code: (sc - 8) as u16, down });
                    }
                }
            }
            _ => {}
        }
    }
}

/// Where the agent is: a direct address, or an ID reached through a relay.
#[derive(Clone)]
enum Target {
    Direct(String),
    Relay { relay: String, id: String },
}

impl Target {
    fn connect(
        &self,
        me: nexdesk_crypto::Identity,
        accept: impl FnOnce(&nexdesk_crypto::IdentityPublic) -> bool,
    ) -> Result<(nexdesk_peer::Reader, Writer, nexdesk_crypto::IdentityPublic), nexdesk_peer::PeerError> {
        match self {
            Target::Direct(a) => client::connect(a, me, accept),
            Target::Relay { relay, id } => client::connect_relay(relay, id, me, accept),
        }
    }

    /// Key under which the agent's identity is pinned.
    fn pin_key(&self) -> String {
        match self {
            Target::Direct(a) => {
                let (h, p) = a.rsplit_once(':').map(|(h, p)| (h, p.parse().ok())).unwrap_or((a.as_str(), None));
                host_key(h, p.unwrap_or(nexdesk_peer::DEFAULT_PORT))
            }
            Target::Relay { id, .. } => host_key(&format!("id-{id}"), nexdesk_peer::DEFAULT_PORT),
        }
    }

    fn label(&self) -> String {
        match self {
            Target::Direct(a) => a.clone(),
            Target::Relay { id, .. } => format!("ID {id}"),
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
    target: Target,
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
                        target.connect(me, |a| a.fingerprint_string() == agent_fp).map_err(|e| e.to_string())
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
    let mut relay = None::<String>;
    let (mut probe, mut clipboard, mut trust) = (false, true, None::<String>);
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--forget" => forget = true,
            "--probe" => probe = true,
            "--relay" => relay = Some(it.next().unwrap_or_else(|| die("--relay needs HOST:PORT"))),
            "--no-clipboard" => clipboard = false,
            "--trust" => trust = Some(it.next().unwrap_or_else(|| die("--trust needs a fingerprint"))),
            "-h" | "--help" => {
                println!("nexdesk-peer-view HOST:PORT [--relay RELAY:PORT] [--forget] [--no-clipboard] [--trust SHA256:..] [--probe]\n  --relay   treat the address as a nine digit ID and reach it through this relay\n  --forget  drop the pinned identity of this agent first\n  --trust   pin the agent if its fingerprint is exactly this\n  --probe   print \"FINGERPRINT <fp>\" and exit (no session)");
                return;
            }
            _ => addr = Some(a),
        }
    }
    let addr = addr.unwrap_or_else(|| die("usage: nexdesk-peer-view HOST:PORT"));
    let dir = store::default_dir().unwrap_or_else(|| die("HOME is not set"));
    let me = store::load_or_create_identity(&dir, "viewer-identity").unwrap_or_else(|e| die(&e.to_string()));
    let mut known = KnownHosts::load(&dir.join("known_agents"));
    let target = match relay {
        Some(r) => {
            if !nexdesk_network::proto::valid_id(&addr) {
                die("with --relay the address must be the agent's nine digit ID");
            }
            let r = if r.contains(':') { r } else { format!("{r}:{}", nexdesk_network::DEFAULT_RELAY_PORT) };
            Target::Relay { relay: r, id: addr.clone() }
        }
        None => Target::Direct(addr.clone()),
    };
    let key = target.pin_key();
    if forget {
        let _ = known.forget(&key);
    }

    let mut pin_error = None;
    let result = target.connect(me, |agent| {
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

    let target_for_retry = target.clone();
    let el = EventLoop::<Ev>::with_user_event().build().unwrap_or_else(|e| die(&e.to_string()));
    let proxy = el.create_proxy();
    let (tx, rx) = mpsc::channel::<Msg>();
    let agent_fp = peer.fingerprint_string();
    std::thread::spawn(move || supervise((reader, writer), rx, proxy, target_for_retry, agent_fp, dir));
    let mut app = App {
        window: None,
        surface: None,
        screen: None,
        out: tx,
        view_only: false,
        title: format!("nexdesk - {}", target.label()),
        cursor: (0.0, 0.0),
        clipboard,
        clip: None,
        actual: false,
        pan: (0, 0),
        fullscreen: false,
        bar_open: false,
        bar_until: None,
        hover: None,
        bar_click: false,
        toast: None,
        monitors: Vec::new(),
        current_monitor: 0,
        mods: ModifiersState::default(),
        swallowed: None,
    };
    let _ = el.run_app(&mut app);
}
