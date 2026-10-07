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
use winit::window::{Window, WindowId};

enum Ev {
    Msg(Msg),
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

fn ask(question: &str) -> bool {
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
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--forget" => forget = true,
            "-h" | "--help" => {
                println!("nexdesk-peer-view HOST:PORT [--forget]\n  --forget  drop the pinned identity of this agent first");
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
        match known.lookup(&key, &fp) {
            Lookup::Match => true,
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
    let (mut reader, mut writer, peer) = match result {
        Ok(x) => x,
        Err(e) => die(&pin_error.unwrap_or_else(|| format!("connection failed: {e}"))),
    };
    eprintln!("connected to {} - waiting for the agent's user to approve if asked", peer.fingerprint_string());

    let el = EventLoop::<Ev>::with_user_event().build().unwrap_or_else(|e| die(&e.to_string()));
    let proxy = el.create_proxy();
    std::thread::spawn(move || loop {
        match reader.recv() {
            Ok(m) => {
                if proxy.send_event(Ev::Msg(m)).is_err() {
                    break;
                }
            }
            Err(e) => {
                let _ = proxy.send_event(Ev::Gone(e.to_string()));
                break;
            }
        }
    });
    let (tx, rx) = mpsc::channel::<Msg>();
    std::thread::spawn(move || {
        let mut w: Writer = writer;
        let mut n = 0u64;
        loop {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(m) => {
                    if w.send(&m).is_err() {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    n += 1;
                    if w.send(&Msg::Ping(n)).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    w.shutdown();
                    break;
                }
            }
        }
    });
    let mut app = App {
        window: None,
        surface: None,
        screen: None,
        out: tx,
        view_only: false,
        title: format!("nexdesk - {addr}"),
        cursor: (0.0, 0.0),
    };
    let _ = el.run_app(&mut app);
}
