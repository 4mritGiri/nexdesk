//! winit application: draws the remote framebuffer with softbuffer and
//! translates window events into RDP fast-path input.
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ironrdp_client::rdp::{RdpInputEvent, RdpOutputEvent};
use ironrdp_input::{
    Database, MouseButton as RdpButton, MousePosition, Operation, Scancode, WheelRotations,
};
use nexdesk_core::scale::{Actual, Fit, View};
use tokio::sync::mpsc::UnboundedSender;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use nexdesk_clipboard::ClipboardHandle;
use crate::tls::CertInfo;
use crate::ui::{self, BarHit, Canvas, Modal, ModalHit, Toast, Toolbar};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Cursor, CursorIcon, CustomCursor, Fullscreen, ResizeDirection, Window, WindowId};

use crate::grab::KeyboardGrab;
use crate::keymap;

pub enum UserEvent {
    Rdp(RdpOutputEvent),
    /// The TLS verifier needs the user to decide about an unknown/changed server certificate.
    CertPrompt(CertInfo, Sender<bool>),
    /// A short message for the toast (sent by background jobs such as the screenshot saver).
    Toast(String),
}

pub struct Options {
    pub title: String,
    pub host_label: String,
    pub initial_size: (u32, u32),
    pub dynamic_resize: bool,
    pub start_fullscreen: bool,
    pub capture_keys: bool,
    /// After a file drop, press Ctrl+V on the remote so the files land in the focused folder.
    pub drop_paste: bool,
    /// Use the system title bar instead of NexDesk's own header bar.
    pub native_frame: bool,
    /// Names for the activity log.
    pub log_profile: String,
    pub log_user: String,
    /// Lets background threads post toasts back to the window.
    pub proxy: EventLoopProxy<UserEvent>,
}

struct Frame {
    buf: Vec<u32>,
    w: u32,
    h: u32,
}

pub struct App {
    title: String,
    initial_size: (u32, u32),
    dynamic_resize: bool,
    input_tx: UnboundedSender<RdpInputEvent>,
    db: Database,
    window: Option<Arc<Window>>,
    context: Option<softbuffer::Context<Arc<Window>>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    frame: Option<Frame>,
    /// Copy of the picture taken when the view was paused.
    frozen: Option<Frame>,
    /// Top-left of the visible part in 1:1 mode.
    pan: (u32, u32),
    pan_tick: Option<Instant>,
    cursor_in: bool,
    proxy: EventLoopProxy<UserEvent>,
    pending_resize: Option<(Instant, u16, u16)>,
    start_fullscreen: bool,
    capture_keys: bool,
    focused: bool,
    grab: KeyboardGrab,
    toolbar: Toolbar,
    bar_was_visible: bool,
    modal: Option<Modal>,
    toast: Option<Toast>,
    cursor: (f64, f64),
    clipboard: Option<ClipboardHandle>,
    drop_paste: bool,
    drop_batch: Vec<PathBuf>,
    drop_deadline: Option<Instant>,
    paste_at: Option<Instant>,
    native_frame: bool,
    log_profile: String,
    log_user: String,
    connected_logged: bool,
    server_cursor: Cursor,
    server_cursor_hidden: bool,
    cursor_overridden: bool,
    last_header_click: Option<Instant>,
    /// Set when the session ended abnormally; `main` turns this into an error exit.
    pub failure: Option<String>,
}

impl App {
    pub fn new(
        o: Options,
        input_tx: UnboundedSender<RdpInputEvent>,
        clipboard: Option<ClipboardHandle>,
    ) -> Self {
        Self {
            title: o.title,
            initial_size: o.initial_size,
            dynamic_resize: o.dynamic_resize,
            input_tx,
            db: Database::new(),
            window: None,
            context: None,
            surface: None,
            frame: None,
            frozen: None,
            pan: (0, 0),
            pan_tick: None,
            cursor_in: false,
            proxy: o.proxy,
            pending_resize: None,
            start_fullscreen: o.start_fullscreen,
            capture_keys: o.capture_keys,
            focused: false,
            grab: KeyboardGrab::new(),
            toolbar: Toolbar::new(o.host_label),
            bar_was_visible: false,
            modal: None,
            toast: None,
            cursor: (0.0, 0.0),
            clipboard,
            drop_paste: o.drop_paste,
            drop_batch: Vec::new(),
            drop_deadline: None,
            paste_at: None,
            native_frame: o.native_frame,
            log_profile: o.log_profile,
            log_user: o.log_user,
            connected_logged: false,
            server_cursor: Cursor::Icon(CursorIcon::Default),
            server_cursor_hidden: false,
            cursor_overridden: false,
            last_header_click: None,
            failure: None,
        }
    }

    fn send_ops(&mut self, ops: impl IntoIterator<Item = Operation>) {
        let events = self.db.apply(ops);
        if !events.is_empty() {
            let _ = self.input_tx.send(RdpInputEvent::FastPath(events));
        }
    }

    fn release_all_keys(&mut self) {
        let events = self.db.release_all();
        if !events.is_empty() {
            let _ = self.input_tx.send(RdpInputEvent::FastPath(events));
        }
    }

    fn is_fullscreen(&self) -> bool {
        self.window
            .as_ref()
            .map(|w| w.fullscreen().is_some())
            .unwrap_or(false)
    }

    /// Capture Super / Alt+Tab etc. only while full screen and focused (mstsc's default).
    fn sync_grab(&mut self) {
        let want = self.capture_keys && self.focused && self.is_fullscreen();
        if let Some(w) = self.window.clone() {
            self.grab.set(&w, want);
        }
    }

    fn toggle_fullscreen(&mut self) {
        // Release keys first so Ctrl/Alt are not left "held" on the remote side.
        self.release_all_keys();
        if let Some(w) = &self.window {
            let next = if w.fullscreen().is_some() {
                None
            } else {
                Some(Fullscreen::Borderless(None))
            };
            w.set_fullscreen(next);
        }
        self.sync_grab();
    }

    /// The picture currently shown (frozen copy while paused).
    fn shown(&self) -> Option<&Frame> {
        self.frozen.as_ref().or(self.frame.as_ref())
    }

    fn view(&self) -> Option<View> {
        let s = self.window.as_ref()?.inner_size();
        let (w, h) = (s.width, s.height.saturating_sub(self.header_h() as u32));
        let f = self.shown()?;
        if self.toolbar.actual {
            Actual::new(f.w, f.h, w, h, self.pan.0, self.pan.1).map(View::Actual)
        } else {
            Fit::new(f.w, f.h, w, h).map(View::Fit)
        }
    }

    /// Direction (-1/0/1 per axis) the 1:1 view should scroll because the pointer is at a window edge.
    fn edge_dir(&self) -> (i64, i64) {
        let (Some(w), Some(View::Actual(a))) = (self.window.as_ref(), self.view()) else { return (0, 0) };
        if !self.cursor_in || self.toolbar.paused || self.modal.is_some() || self.bar_hit() != BarHit::None {
            return (0, 0);
        }
        let s = w.inner_size();
        let (aw, ah) = (f64::from(s.width), f64::from(s.height) - f64::from(self.header_h()));
        let (x, y) = (self.cursor.0, self.cursor.1 - f64::from(self.header_h()));
        if y < 0.0 || self.resize_edge().is_some() {
            return (0, 0);
        }
        let e = 18.0 * f64::from(self.ui_scale());
        let (mx, my) = a.max_pan();
        let dx = if mx > 0 && x < e { -1 } else if mx > 0 && x >= aw - e { 1 } else { 0 };
        let dy = if my > 0 && y < e { -1 } else if my > 0 && y >= ah - e { 1 } else { 0 };
        (dx, dy)
    }

    /// Scroll the 1:1 view one step; returns whether it moved.
    fn pan_step(&mut self, dx: i64, dy: i64) -> bool {
        let Some(View::Actual(a)) = self.view() else { return false };
        let (mx, my) = a.max_pan();
        let step = 24 * i64::from(self.ui_scale());
        let nx = (i64::from(self.pan.0) + dx * step).clamp(0, i64::from(mx)) as u32;
        let ny = (i64::from(self.pan.1) + dy * step).clamp(0, i64::from(my)) as u32;
        let moved = (nx, ny) != self.pan;
        self.pan = (nx, ny);
        moved
    }

    fn set_paused(&mut self, on: bool) {
        if on == self.toolbar.paused {
            return;
        }
        self.release_all_keys();
        self.toolbar.paused = on;
        self.frozen = if on {
            self.frame.as_ref().map(|f| Frame { buf: f.buf.clone(), w: f.w, h: f.h })
        } else {
            None
        };
        self.show_toast(if on { "View paused: input is not sent" } else { "Resumed" });
    }

    fn send_cad(&mut self) {
        if self.toolbar.paused {
            self.show_toast("Resume first to send Ctrl+Alt+Del");
            return;
        }
        self.release_all_keys();
        let (ctrl, alt, del) = (Scancode::from_u16(0x1D), Scancode::from_u16(0x38), Scancode::from_u16(0xE053));
        self.send_ops([
            Operation::KeyPressed(ctrl),
            Operation::KeyPressed(alt),
            Operation::KeyPressed(del),
            Operation::KeyReleased(del),
            Operation::KeyReleased(alt),
            Operation::KeyReleased(ctrl),
        ]);
        self.show_toast("Sent Ctrl+Alt+Del");
    }

    fn screenshot(&mut self) {
        let Some(f) = self.shown() else {
            self.show_toast("Nothing to capture yet");
            return;
        };
        let (buf, w, h) = (f.buf.clone(), f.w, f.h);
        let host = self.toolbar.title.clone();
        let proxy = self.proxy.clone();
        let spawned = std::thread::Builder::new().name("screenshot".into()).spawn(move || {
            let msg = match crate::shot::save(&buf, w, h, &host) {
                Ok(p) => format!("Screenshot saved: {}", p.display()),
                Err(e) => format!("Screenshot failed: {e}"),
            };
            let _ = proxy.send_event(UserEvent::Toast(msg));
        });
        if spawned.is_err() {
            self.show_toast("Screenshot failed: cannot start worker");
        }
    }

    /// UI scale for overlay widgets (1 on normal screens, 2 on HiDPI).
    fn ui_scale(&self) -> i32 {
        self.window.as_ref().map(|w| w.scale_factor().round() as i32).unwrap_or(1).clamp(1, 4)
    }

    fn redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// Windowed mode with our own header bar (no system title bar).
    fn docked(&self) -> bool {
        !self.native_frame && !self.is_fullscreen()
    }

    /// Height of the docked header in physical pixels (0 when not docked).
    fn header_h(&self) -> i32 {
        if self.docked() { 40 * self.ui_scale() } else { 0 }
    }

    fn bar_visible(&self) -> bool {
        self.docked() || (self.is_fullscreen() && self.modal.is_none() && self.toolbar.visible())
    }

    /// Window edge under the pointer, for resizing the undecorated window.
    fn resize_edge(&self) -> Option<ResizeDirection> {
        let w = self.window.as_ref()?;
        if !self.docked() || w.is_maximized() || self.modal.is_some() {
            return None;
        }
        let s = w.inner_size();
        let e = f64::from(6 * self.ui_scale());
        let (x, y) = self.cursor;
        let (l, r) = (x < e, x >= f64::from(s.width) - e);
        let (t, b) = (y < e, y >= f64::from(s.height) - e);
        Some(match (l, r, t, b) {
            (true, _, true, _) => ResizeDirection::NorthWest,
            (_, true, true, _) => ResizeDirection::NorthEast,
            (true, _, _, true) => ResizeDirection::SouthWest,
            (_, true, _, true) => ResizeDirection::SouthEast,
            (true, ..) => ResizeDirection::West,
            (_, true, ..) => ResizeDirection::East,
            (_, _, true, _) => ResizeDirection::North,
            (_, _, _, true) => ResizeDirection::South,
            _ => return None,
        })
    }

    /// Pointer cursor over buttons, resize cursors on edges, otherwise the server's cursor.
    fn update_cursor(&mut self) {
        let hit = self.bar_hit();
        let edge = self.resize_edge();
        let want: Option<CursorIcon> = match (hit, edge) {
            (_, Some(d)) => Some(match d {
                ResizeDirection::North | ResizeDirection::South => CursorIcon::NsResize,
                ResizeDirection::East | ResizeDirection::West => CursorIcon::EwResize,
                ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::NwseResize,
                ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::NeswResize,
            }),
            (BarHit::Bar, None) => Some(CursorIcon::Default),
            (BarHit::None, None) => None,
            (_, None) => Some(CursorIcon::Pointer), // every button is clickable
        };
        let Some(w) = self.window.clone() else { return };
        match want {
            Some(icon) => {
                w.set_cursor_visible(true);
                w.set_cursor(icon);
                self.cursor_overridden = true;
            }
            None if self.cursor_overridden => {
                self.cursor_overridden = false;
                self.apply_server_cursor();
            }
            None => {}
        }
    }

    fn apply_server_cursor(&self) {
        if self.cursor_overridden {
            return;
        }
        if let Some(w) = &self.window {
            if self.server_cursor_hidden {
                w.set_cursor_visible(false);
            } else {
                w.set_cursor_visible(true);
                w.set_cursor(self.server_cursor.clone());
            }
        }
    }

    fn bar_hit(&self) -> BarHit {
        if !self.bar_visible() {
            return BarHit::None;
        }
        let w = self.window.as_ref().map(|w| w.inner_size().width as i32).unwrap_or(0);
        if self.modal.is_some() {
            return BarHit::None;
        }
        self.toolbar.hit(w, self.ui_scale(), self.cursor.0, self.cursor.1)
    }

    fn modal_hit(&self) -> ModalHit {
        match (&self.modal, &self.window) {
            (Some(m), Some(w)) => {
                let s = w.inner_size();
                m.hit(s.width as i32, s.height as i32, self.ui_scale(), self.cursor.0, self.cursor.1)
            }
            _ => ModalHit::None,
        }
    }

    fn show_toast(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast { text: text.into(), until: Instant::now() + Duration::from_secs(4) });
        self.redraw();
    }

    fn close_session(&mut self, el: &ActiveEventLoop) {
        nexdesk_core::logs::connection(nexdesk_core::logs::Level::Info, "Disconnected", &self.log_profile, &self.toolbar.title, &self.log_user, "closed by user");
        let _ = self.input_tx.send(RdpInputEvent::Close);
        el.exit();
    }

    fn resolve_modal(&mut self, accept: bool, el: &ActiveEventLoop) {
        if let Some(m) = self.modal.take() {
            let was_error = m.reply.is_none();
            if let Some(r) = m.reply {
                let _ = r.send(accept);
            }
            if was_error {
                el.exit(); // a plain error box: closing it ends the program
            }
            self.redraw();
        }
    }

    fn show_error(&mut self, el: &ActiveEventLoop, heading: &str, msg: &str) {
        if self.window.is_none() {
            el.exit();
            return;
        }
        self.modal = Some(Modal {
            heading: heading.to_owned(),
            lines: ui::wrap(msg, 90).into_iter().map(|l| (l, ui::FG)).collect(),
            accent: ui::DANGER,
            accept: None,
            reject: "Close".into(),
            reply: None,
        });
        self.redraw();
    }

    fn bar_action(&mut self, hit: BarHit, el: &ActiveEventLoop) {
        match hit {
            BarHit::Pin => {
                self.toolbar.pinned = !self.toolbar.pinned;
                self.toolbar.touch();
            }
            BarHit::Minimize => {
                self.release_all_keys();
                if let Some(w) = &self.window {
                    w.set_minimized(true);
                }
            }
            BarHit::Restore => {
                self.toolbar.hide_now();
                self.toggle_fullscreen();
            }
            BarHit::Close => self.close_session(el),
            BarHit::Cad => self.send_cad(),
            BarHit::Shot => self.screenshot(),
            BarHit::Scale => {
                self.toolbar.actual = !self.toolbar.actual;
                self.pan = (0, 0);
                self.show_toast(if self.toolbar.actual { "Actual size: move the pointer to a window edge to scroll" } else { "Fit to window" });
            }
            BarHit::Pause => self.set_paused(!self.toolbar.paused),
            _ => {}
        }
        self.redraw();
    }

    fn draw(&mut self) {
        let u = self.ui_scale();
        let hover = self.bar_hit();
        let modal_hover = self.modal_hit();
        let show_bar = self.bar_visible();
        let view = self.view();
        let pan = self.pan;
        let (Some(window), Some(surface)) = (self.window.as_ref(), self.surface.as_mut()) else {
            return;
        };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        if surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        let hb = (if self.native_frame || window.fullscreen().is_some() { 0 } else { 40 * u }) as u32;
        let hb = hb.min(size.height);
        let src = self.frozen.as_ref().or(self.frame.as_ref());
        match (src, view) {
            (Some(frame), Some(v)) => {
                let skip = ((hb as usize) * (size.width as usize)).min(buffer.len());
                let (top, rest) = buffer.split_at_mut(skip);
                top.fill(ui::BG);
                v.blit(&frame.buf, rest);
            }
            _ => buffer.fill(ui::BG),
        }
        {
            let mut c = Canvas { buf: &mut buffer, w: size.width as usize, h: size.height as usize };
            if self.frame.is_none() && self.modal.is_none() {
                let msg = "Connecting...";
                let k = 3 * u;
                let tx = (size.width as i32 - ui::text_width(msg, k)) / 2;
                c.text(tx, size.height as i32 / 2 - 4 * k, msg, k, ui::DIM);
            }
            if let Some(View::Actual(a)) = &view {
                // slim scroll indicators for the 1:1 view
                let (mx, my) = a.max_pan();
                let hb = hb as i32;
                let (aw, ah) = (a.dst_w as i32, a.dst_h as i32);
                if mx > 0 {
                    let thumb = (aw as i64 * i64::from(a.dst_w) / i64::from(a.src_w)).max(24) as i32;
                    let x = (i64::from(pan.0.min(mx)) * i64::from((aw - thumb).max(0)) / i64::from(mx)) as i32;
                    c.rrect(x, hb + ah - 6 * u, thumb, 4 * u, 2 * u, ui::DIM);
                }
                if my > 0 {
                    let thumb = (ah as i64 * i64::from(a.dst_h) / i64::from(a.src_h)).max(24) as i32;
                    let y = (i64::from(pan.1.min(my)) * i64::from((ah - thumb).max(0)) / i64::from(my)) as i32;
                    c.rrect(aw - 6 * u, hb + y, 4 * u, thumb, 2 * u, ui::DIM);
                }
            }
            if show_bar {
                self.toolbar.draw(&mut c, u, hover);
            }
            if let Some(t) = &self.toast {
                t.draw(&mut c, u);
            }
            if let Some(m) = &self.modal {
                m.draw(&mut c, u, modal_hover);
            }
        }
        let _ = buffer.present();
    }

    fn handle_key(&mut self, code: KeyCode, state: ElementState) {
        if self.toolbar.paused {
            return;
        }
        let ctrl = self.db.is_key_pressed(Scancode::from_u16(0x1D));
        let alt = self.db.is_key_pressed(Scancode::from_u16(0x38));
        // Local hotkey: Ctrl+Alt+Break toggles full screen (same as mstsc).
        if code == KeyCode::Pause && state == ElementState::Pressed && ctrl && alt {
            self.toggle_fullscreen();
            return;
        }
        // Local hotkey: Ctrl+Alt+End sends Ctrl+Alt+Del (the real one is grabbed by the local OS).
        if code == KeyCode::End && state == ElementState::Pressed {
            if ctrl && alt {
                let del = Scancode::from_u16(0xE053);
                self.send_ops([Operation::KeyPressed(del), Operation::KeyReleased(del)]);
                return;
            }
        }
        if let Some(sc) = keymap::scancode(code) {
            let op = match state {
                ElementState::Pressed => Operation::KeyPressed(sc),
                ElementState::Released => Operation::KeyReleased(sc),
            };
            self.send_ops([op]);
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(self.title.clone())
            .with_decorations(self.native_frame)
            .with_inner_size(LogicalSize::new(self.initial_size.0, self.initial_size.1))
            .with_fullscreen(
                self.start_fullscreen
                    .then_some(Fullscreen::Borderless(None)),
            );
        // Window class / app id: lets the dock match the window to the .desktop entry (name + icon).
        #[cfg(target_os = "linux")]
        let attrs = {
            use winit::platform::wayland::WindowAttributesExtWayland;
            use winit::platform::x11::WindowAttributesExtX11;
            let attrs = WindowAttributesExtWayland::with_name(attrs, "nexdesk", "nexdesk");
            WindowAttributesExtX11::with_name(attrs, "nexdesk", "nexdesk")
        };
        let window = match el.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.failure = Some(format!("cannot create window: {e}"));
                el.exit();
                return;
            }
        };
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface =
            softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");
        self.window = Some(window);
        self.context = Some(context);
        self.surface = Some(surface);
    }

    fn user_event(&mut self, el: &ActiveEventLoop, event: UserEvent) {
        let ev = match event {
            UserEvent::Rdp(ev) => ev,
            UserEvent::Toast(t) => {
                self.show_toast(t);
                return;
            }
            UserEvent::CertPrompt(info, reply) => {
                let changed = info.pinned.is_some();
                let mut lines: Vec<(String, u32)> = Vec::new();
                if changed {
                    lines.push(("The certificate of this computer is DIFFERENT from the one you trusted before.".into(), ui::DANGER));
                    lines.push(("Someone may be intercepting the connection. Only continue if the server was reinstalled.".into(), ui::DANGER));
                } else {
                    lines.push(("The identity of the remote computer could not be verified.".into(), ui::WARN));
                    lines.push(("Connect only if you recognise the fingerprint below.".into(), ui::DIM));
                }
                lines.push((String::new(), ui::FG));
                lines.push((format!("Computer : {}", info.host), ui::FG));
                lines.push((format!("Subject  : {}", info.subject), ui::FG));
                lines.push(("Fingerprint:".into(), ui::FG));
                for l in ui::wrap(&info.fingerprint, 44) {
                    lines.push((format!("  {l}"), ui::ACCENT));
                }
                if let Some(old) = &info.pinned {
                    lines.push(("Previously trusted:".into(), ui::FG));
                    for l in ui::wrap(old, 44) {
                        lines.push((format!("  {l}"), ui::DIM));
                    }
                }
                self.modal = Some(Modal {
                    heading: if changed { "SERVER CERTIFICATE CHANGED".into() } else { "Unknown server certificate".into() },
                    lines,
                    accent: if changed { ui::DANGER } else { ui::WARN },
                    accept: Some(if changed { "Trust new certificate".into() } else { "Trust and connect".into() }),
                    reject: "Cancel".into(),
                    reply: Some(reply),
                });
                if let Some(w) = &self.window {
                    w.focus_window();
                }
                self.redraw();
                return;
            }
        };
        match ev {
            RdpOutputEvent::Image {
                buffer,
                width,
                height,
            } => {
                if !self.connected_logged {
                    self.connected_logged = true;
                    nexdesk_core::logs::connection(nexdesk_core::logs::Level::Info, "Connected", &self.log_profile, &self.toolbar.title, &self.log_user, "");
                }
                self.frame = Some(Frame {
                    buf: buffer,
                    w: u32::from(width.get()),
                    h: u32::from(height.get()),
                });
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            RdpOutputEvent::PointerHidden => {
                self.server_cursor_hidden = true;
                self.apply_server_cursor();
            }
            RdpOutputEvent::PointerDefault => {
                self.server_cursor_hidden = false;
                self.server_cursor = Cursor::Icon(CursorIcon::Default);
                self.apply_server_cursor();
            }
            RdpOutputEvent::PointerBitmap(p) => {
                // The server sends premultiplied RGBA; winit wants straight alpha.
                let mut rgba = p.bitmap_data.clone();
                for px in rgba.chunks_exact_mut(4) {
                    let a = u32::from(px[3]);
                    if a != 0 && a != 255 {
                        for c in &mut px[..3] {
                            *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                        }
                    }
                }
                let (w, h) = (p.width, p.height);
                let (hx, hy) = (p.hotspot_x.min(w.saturating_sub(1)), p.hotspot_y.min(h.saturating_sub(1)));
                match CustomCursor::from_rgba(rgba, w, h, hx, hy) {
                    Ok(src) => {
                        self.server_cursor = Cursor::Custom(el.create_custom_cursor(src));
                        self.server_cursor_hidden = false;
                        self.apply_server_cursor();
                    }
                    Err(e) => {
                        tracing::debug!("bad pointer bitmap: {e}");
                        self.server_cursor = Cursor::Icon(CursorIcon::Default);
                        self.server_cursor_hidden = false;
                        self.apply_server_cursor();
                    }
                }
            }
            RdpOutputEvent::PointerPosition { .. } => {}
            RdpOutputEvent::ConnectionFailure(e) => {
                let msg = crate::diag::explain("connection failed", &e);
                nexdesk_core::logs::connection(nexdesk_core::logs::Level::Error, "Failed", &self.log_profile, &self.toolbar.title, &self.log_user, &msg);
                self.failure = Some(msg.clone());
                self.show_error(el, "Connection failed", &msg);
            }
            RdpOutputEvent::Terminated(res) => {
                match res {
                    Ok(reason) => {
                        eprintln!("session ended: {reason:?}");
                        nexdesk_core::logs::connection(nexdesk_core::logs::Level::Info, "Disconnected", &self.log_profile, &self.toolbar.title, &self.log_user, &format!("{reason:?}"));
                    }
                    Err(e) => {
                        let c = format!("{e:#}");
                        let msg = match crate::diag::hint(&c) { Some(h) => format!("session error: {c}\n\n{h}"), None => format!("session error: {c}") };
                        nexdesk_core::logs::connection(nexdesk_core::logs::Level::Error, "Failed", &self.log_profile, &self.toolbar.title, &self.log_user, &msg);
                        self.failure = Some(msg.clone());
                        self.show_error(el, "Session ended", &msg);
                        return;
                    }
                }
                el.exit();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        self.toolbar.docked = self.docked();
        match event {
            WindowEvent::CloseRequested => {
                if let Some(m) = self.modal.take() {
                    if let Some(r) = m.reply {
                        let _ = r.send(false);
                    }
                }
                self.close_session(el);
            }
            WindowEvent::DroppedFile(path) => {
                self.drop_batch.push(path);
                self.drop_deadline = Some(Instant::now() + Duration::from_millis(250));
            }
            WindowEvent::CursorEntered { .. } => self.cursor_in = true,
            WindowEvent::CursorLeft { .. } => {
                self.cursor_in = false;
                self.pan_tick = None;
                if self.bar_visible() && !self.toolbar.pinned {
                    self.toolbar.touch();
                    self.redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                self.toolbar.docked = self.docked();
                self.draw()
            }
            WindowEvent::Resized(size) => {
                self.sync_grab(); // leaving/entering full screen via the window manager
                if self.dynamic_resize && size.width > 0 && size.height > 0 {
                    let w = (size.width.min(8192) & !1) as u16; // width must be even
                    let h = size.height.saturating_sub(self.header_h() as u32).min(8192) as u16;
                    self.pending_resize = Some((Instant::now() + Duration::from_millis(400), w, h));
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::Focused(focused) => {
                self.focused = focused;
                if !focused {
                    self.release_all_keys();
                }
                self.sync_grab();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if self.modal.is_some() {
                    if event.state == ElementState::Pressed {
                        match event.physical_key {
                            PhysicalKey::Code(KeyCode::Enter) | PhysicalKey::Code(KeyCode::NumpadEnter) => {
                                let accept = self.modal.as_ref().map(|m| m.accept.is_some()).unwrap_or(false);
                                // Enter only accepts when the dialog has an accept button and the user
                                // is not looking at a "changed certificate" warning (those need a click).
                                let dangerous = self.modal.as_ref().map(|m| m.accent == ui::DANGER).unwrap_or(false);
                                self.resolve_modal(accept && !dangerous, el);
                            }
                            PhysicalKey::Code(KeyCode::Escape) => self.resolve_modal(false, el),
                            _ => {}
                        }
                    }
                    return;
                }
                if event.repeat {
                    return; // the server generates its own key repeat
                }
                if let PhysicalKey::Code(code) = event.physical_key {
                    self.handle_key(code, event.state);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                self.update_cursor();
                if self.modal.is_some() {
                    self.redraw();
                    return;
                }
                if self.is_fullscreen() {
                    let u = f64::from(self.ui_scale());
                    if position.y <= 1.0 * u {
                        self.toolbar.touch(); // pushing the pointer to the top edge reveals the bar
                        self.redraw();
                    }
                    if self.bar_hit() != BarHit::None {
                        self.toolbar.touch();
                        self.redraw();
                        return; // the bar swallows pointer movement
                    }
                    if self.toolbar.visible() {
                        self.redraw(); // hover highlight cleared
                    }
                } else if self.docked() {
                    self.redraw(); // hover glyphs on the dots
                    if self.bar_hit() != BarHit::None || self.resize_edge().is_some() {
                        return;
                    }
                }
                self.cursor_in = true;
                if self.toolbar.actual && self.pan_tick.is_none() && self.edge_dir() != (0, 0) {
                    self.pan_tick = Some(Instant::now());
                }
                if self.toolbar.paused {
                    return;
                }
                if let Some(view) = self.view() {
                    let (x, y) = view.to_remote(position.x, position.y - f64::from(self.header_h()));
                    self.send_ops([Operation::MouseMove(MousePosition { x, y })]);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if self.modal.is_some() {
                    if state == ElementState::Released && button == MouseButton::Left {
                        match self.modal_hit() {
                            ModalHit::Accept => self.resolve_modal(true, el),
                            ModalHit::Reject => self.resolve_modal(false, el),
                            ModalHit::None => {}
                        }
                    }
                    return;
                }
                if state == ElementState::Pressed && button == MouseButton::Left {
                    if let (Some(dir), Some(w)) = (self.resize_edge(), self.window.as_ref()) {
                        let _ = w.drag_resize_window(dir);
                        return;
                    }
                }
                let hit = self.bar_hit();
                if hit != BarHit::None {
                    if state == ElementState::Pressed && button == MouseButton::Left && hit == BarHit::Bar && self.docked() {
                        // empty part of the header: drag the window, double-click maximises
                        let now = Instant::now();
                        let double = self.last_header_click.map(|t| now.duration_since(t) < Duration::from_millis(400)).unwrap_or(false);
                        self.last_header_click = Some(now);
                        if let Some(w) = &self.window {
                            if double {
                                w.set_maximized(!w.is_maximized());
                            } else {
                                let _ = w.drag_window();
                            }
                        }
                    }
                    if state == ElementState::Released && button == MouseButton::Left {
                        self.bar_action(hit, el);
                    }
                    return;
                }
                if self.toolbar.paused {
                    return;
                }
                let b = match button {
                    MouseButton::Left => RdpButton::Left,
                    MouseButton::Right => RdpButton::Right,
                    MouseButton::Middle => RdpButton::Middle,
                    MouseButton::Back => RdpButton::X1,
                    MouseButton::Forward => RdpButton::X2,
                    MouseButton::Other(_) => return,
                };
                let op = match state {
                    ElementState::Pressed => Operation::MouseButtonPressed(b),
                    ElementState::Released => Operation::MouseButtonReleased(b),
                };
                self.send_ops([op]);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.modal.is_some() || self.bar_hit() != BarHit::None || self.toolbar.paused {
                    return;
                }
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * 120.0, y * 120.0),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                let mut ops = Vec::new();
                if dy != 0.0 {
                    ops.push(Operation::WheelRotations(WheelRotations {
                        is_vertical: true,
                        rotation_units: dy.round().clamp(-32768.0, 32767.0) as i16,
                    }));
                }
                if dx != 0.0 {
                    ops.push(Operation::WheelRotations(WheelRotations {
                        is_vertical: false,
                        rotation_units: dx.round().clamp(-32768.0, 32767.0) as i16,
                    }));
                }
                self.send_ops(ops);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        let now = Instant::now();

        // Debounced dynamic resize: only tell the server once the user stops dragging.
        if let Some((deadline, w, h)) = self.pending_resize {
            if now >= deadline {
                self.pending_resize = None;
                let _ = self.input_tx.send(RdpInputEvent::Resize {
                    width: w,
                    height: h,
                    scale_factor: 100,
                    physical_size: None,
                });
            }
        }

        // Dropped files: hand the whole batch to the clipboard once the drop settles.
        if self.drop_deadline.map(|d| now >= d).unwrap_or(false) {
            self.drop_deadline = None;
            let files = std::mem::take(&mut self.drop_batch);
            let n = files.len();
            let ok = self.clipboard.as_ref().map(|c| c.offer_files(files)).unwrap_or(false);
            if ok && self.drop_paste {
                self.paste_at = Some(now + Duration::from_millis(600));
                self.show_toast(format!("Sending {n} file(s) to the remote desktop..."));
            } else if ok {
                self.show_toast(format!("{n} file(s) ready: press Ctrl+V on the remote desktop"));
            } else {
                self.show_toast("Drop failed: clipboard redirection is not available");
            }
        }
        // Ctrl+V on the remote, after the file list has had time to reach the server.
        if self.paste_at.map(|d| now >= d).unwrap_or(false) {
            self.paste_at = None;
            let (ctrl, v) = (Scancode::from_u16(0x1D), Scancode::from_u16(0x2F));
            self.send_ops([
                Operation::KeyPressed(ctrl),
                Operation::KeyPressed(v),
                Operation::KeyReleased(v),
                Operation::KeyReleased(ctrl),
            ]);
        }

        // 1:1 view: scroll while the pointer rests at a window edge.
        if self.pan_tick.map(|d| now >= d).unwrap_or(false) {
            let (dx, dy) = self.edge_dir();
            self.pan_tick = None;
            if (dx, dy) != (0, 0) {
                if self.pan_step(dx, dy) {
                    if let (Some(view), false) = (self.view(), self.toolbar.paused) {
                        let (x, y) = view.to_remote(self.cursor.0, self.cursor.1 - f64::from(self.header_h()));
                        self.send_ops([Operation::MouseMove(MousePosition { x, y })]);
                    }
                    self.redraw();
                    self.pan_tick = Some(now + Duration::from_millis(16));
                }
            }
        }

        if self.toast.as_ref().map(|t| now >= t.until).unwrap_or(false) {
            self.toast = None;
            self.redraw();
        }
        // Repaint when the connection bar appears/disappears (auto-hide timer expiry).
        let bar_now = self.bar_visible();
        if bar_now != self.bar_was_visible {
            self.bar_was_visible = bar_now;
            self.redraw();
        }

        let deadlines = [
            self.pending_resize.map(|(d, _, _)| d),
            self.drop_deadline,
            self.paste_at,
            self.toast.as_ref().map(|t| t.until),
            self.toolbar.hide_deadline(),
            self.pan_tick,
        ];
        match deadlines.into_iter().flatten().min() {
            Some(d) => {
                el.set_control_flow(ControlFlow::WaitUntil(d + Duration::from_millis(5)));
            }
            None => el.set_control_flow(ControlFlow::Wait),
        }
    }
}
