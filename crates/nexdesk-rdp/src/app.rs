//! winit application: draws the remote framebuffer with softbuffer and
//! translates window events into RDP fast-path input.
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ironrdp_client::rdp::{RdpInputEvent, RdpOutputEvent};
use ironrdp_input::{Database, MouseButton as RdpButton, MousePosition, Operation, Scancode, WheelRotations};
use nexdesk_core::scale::{blit_fit, Fit};
use tokio::sync::mpsc::UnboundedSender;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::keymap;

pub enum UserEvent {
    Rdp(RdpOutputEvent),
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
    /// Set when the session ended abnormally; `main` turns this into an error exit.
    pub failure: Option<String>,
}

impl App {
    pub fn new(
        title: String,
        initial_size: (u32, u32),
        dynamic_resize: bool,
        input_tx: UnboundedSender<RdpInputEvent>,
    ) -> Self {
        Self {
            title,
            initial_size,
            dynamic_resize,
            input_tx,
            db: Database::new(),
            window: None,
            context: None,
            surface: None,
            frame: None,
            pending_resize: None,
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

    fn fit(&self) -> Option<Fit> {
        let (w, h) = {
            let s = self.window.as_ref()?.inner_size();
            (s.width, s.height)
        };
        let f = self.frame.as_ref()?;
        Fit::new(f.w, f.h, w, h)
    }

    fn draw(&mut self) {
        let (Some(window), Some(surface), Some(frame)) =
            (self.window.as_ref(), self.surface.as_mut(), self.frame.as_ref())
        else {
            return;
        };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        if surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else { return };
        blit_fit(&frame.buf, frame.w, frame.h, &mut buffer, size.width, size.height);
        let _ = buffer.present();
    }

    fn handle_key(&mut self, code: KeyCode, state: ElementState) {
        // Local hotkey: Ctrl+Alt+End sends Ctrl+Alt+Del (the real one is grabbed by the local OS).
        if code == KeyCode::End && state == ElementState::Pressed {
            let ctrl = self.db.is_key_pressed(Scancode::from_u16(0x1D));
            let alt = self.db.is_key_pressed(Scancode::from_u16(0x38));
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
            .with_inner_size(LogicalSize::new(self.initial_size.0, self.initial_size.1));
        let window = match el.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.failure = Some(format!("cannot create window: {e}"));
                el.exit();
                return;
            }
        };
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface = softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");
        self.window = Some(window);
        self.context = Some(context);
        self.surface = Some(surface);
    }

    fn user_event(&mut self, el: &ActiveEventLoop, event: UserEvent) {
        let UserEvent::Rdp(ev) = event;
        match ev {
            RdpOutputEvent::Image { buffer, width, height } => {
                self.frame = Some(Frame { buf: buffer, w: u32::from(width.get()), h: u32::from(height.get()) });
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
            // Server-drawn cursor shapes are not rendered yet (see docs/ROADMAP.md).
            RdpOutputEvent::PointerBitmap(_) | RdpOutputEvent::PointerPosition { .. } => {}
            RdpOutputEvent::ConnectionFailure(e) => {
                self.failure = Some(format!("connection failed: {e}"));
                el.exit();
            }
            RdpOutputEvent::Terminated(res) => {
                match res {
                    Ok(reason) => eprintln!("session ended: {reason:?}"),
                    Err(e) => self.failure = Some(format!("session error: {e}")),
                }
                el.exit();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                let _ = self.input_tx.send(RdpInputEvent::Close);
                el.exit();
            }
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::Resized(size) => {
                if self.dynamic_resize && size.width > 0 && size.height > 0 {
                    let w = (size.width.min(8192) & !1) as u16; // width must be even
                    let h = size.height.min(8192) as u16;
                    self.pending_resize = Some((Instant::now() + Duration::from_millis(400), w, h));
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::Focused(false) => self.release_all_keys(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.repeat {
                    return; // the server generates its own key repeat
                }
                if let PhysicalKey::Code(code) = event.physical_key {
                    self.handle_key(code, event.state);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(fit) = self.fit() {
                    let (x, y) = fit.to_remote(position.x, position.y);
                    self.send_ops([Operation::MouseMove(MousePosition { x, y })]);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
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
        // Debounced dynamic resize: only tell the server once the user stops dragging.
        if let Some((deadline, w, h)) = self.pending_resize {
            if Instant::now() >= deadline {
                self.pending_resize = None;
                let _ = self.input_tx.send(RdpInputEvent::Resize {
                    width: w,
                    height: h,
                    scale_factor: 100,
                    physical_size: None,
                });
                el.set_control_flow(ControlFlow::Wait);
            } else {
                el.set_control_flow(ControlFlow::WaitUntil(deadline));
            }
        }
    }
}
