//! winit application: draws the remote framebuffer with softbuffer and
//! translates window events into RDP fast-path input.
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ironrdp_client::rdp::{RdpInputEvent, RdpOutputEvent};
use ironrdp_input::{
    Database, MouseButton as RdpButton, MousePosition, Operation, Scancode, WheelRotations,
};
use nexdesk_core::scale::{blit_fit, Fit};
use tokio::sync::mpsc::UnboundedSender;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use nexdesk_clipboard::ClipboardHandle;
use crate::tls::CertInfo;
use crate::ui::{self, BarHit, Canvas, Modal, ModalHit, Toast, Toolbar};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Cursor, CursorIcon, CustomCursor, Fullscreen, Window, WindowId};

use crate::grab::KeyboardGrab;
use crate::keymap;

pub enum UserEvent {
    Rdp(RdpOutputEvent),
    /// The TLS verifier needs the user to decide about an unknown/changed server certificate.
    CertPrompt(CertInfo, Sender<bool>),
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

    fn fit(&self) -> Option<Fit> {
        let (w, h) = {
            let s = self.window.as_ref()?.inner_size();
            (s.width, s.height)
        };
        let f = self.frame.as_ref()?;
        Fit::new(f.w, f.h, w, h)
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

    fn bar_visible(&self) -> bool {
        self.is_fullscreen() && self.modal.is_none() && self.toolbar.visible()
    }

    fn bar_hit(&self) -> BarHit {
        if !self.bar_visible() {
            return BarHit::None;
        }
        let w = self.window.as_ref().map(|w| w.inner_size().width as i32).unwrap_or(0);
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
            _ => {}
        }
        self.redraw();
    }

    fn draw(&mut self) {
        let u = self.ui_scale();
        let hover = self.bar_hit();
        let modal_hover = self.modal_hit();
        let show_bar = self.bar_visible();
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
        match self.frame.as_ref() {
            Some(frame) => blit_fit(&frame.buf, frame.w, frame.h, &mut buffer, size.width, size.height),
            None => buffer.fill(ui::BG),
        }
        {
            let mut c = Canvas { buf: &mut buffer, w: size.width as usize, h: size.height as usize };
            if self.frame.is_none() && self.modal.is_none() {
                let msg = "Connecting...";
                let k = 3 * u;
                let tx = (size.width as i32 - ui::text_width(msg, k)) / 2;
                c.text(tx, size.height as i32 / 2 - 4 * k, msg, k, ui::DIM);
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
            .with_inner_size(LogicalSize::new(self.initial_size.0, self.initial_size.1))
            .with_fullscreen(
                self.start_fullscreen
                    .then_some(Fullscreen::Borderless(None)),
            );
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
                if let Some(w) = &self.window {
                    w.set_cursor_visible(false);
                }
            }
            RdpOutputEvent::PointerDefault => {
                if let Some(w) = &self.window {
                    w.set_cursor_visible(true);
                    w.set_cursor(CursorIcon::Default);
                }
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
                        let cursor = el.create_custom_cursor(src);
                        if let Some(win) = &self.window {
                            win.set_cursor(Cursor::Custom(cursor));
                            win.set_cursor_visible(true);
                        }
                    }
                    Err(e) => {
                        tracing::debug!("bad pointer bitmap: {e}");
                        if let Some(win) = &self.window {
                            win.set_cursor_visible(true);
                            win.set_cursor(CursorIcon::Default);
                        }
                    }
                }
            }
            RdpOutputEvent::PointerPosition { .. } => {}
            RdpOutputEvent::ConnectionFailure(e) => {
                let msg = format!("connection failed: {e}");
                self.failure = Some(msg.clone());
                self.show_error(el, "Connection failed", &msg);
            }
            RdpOutputEvent::Terminated(res) => {
                match res {
                    Ok(reason) => eprintln!("session ended: {reason:?}"),
                    Err(e) => {
                        let msg = format!("session error: {e}");
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
            WindowEvent::CursorLeft { .. } => {
                if self.bar_visible() && !self.toolbar.pinned {
                    self.toolbar.touch();
                    self.redraw();
                }
            }
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::Resized(size) => {
                self.sync_grab(); // leaving/entering full screen via the window manager
                if self.dynamic_resize && size.width > 0 && size.height > 0 {
                    let w = (size.width.min(8192) & !1) as u16; // width must be even
                    let h = size.height.min(8192) as u16;
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
                }
                if let Some(fit) = self.fit() {
                    let (x, y) = fit.to_remote(position.x, position.y);
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
                let hit = self.bar_hit();
                if hit != BarHit::None {
                    if state == ElementState::Released && button == MouseButton::Left {
                        self.bar_action(hit, el);
                    }
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
                if self.modal.is_some() || self.bar_hit() != BarHit::None {
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
        ];
        match deadlines.into_iter().flatten().min() {
            Some(d) => {
                el.set_control_flow(ControlFlow::WaitUntil(d + Duration::from_millis(5)));
            }
            None => el.set_control_flow(ControlFlow::Wait),
        }
    }
}
