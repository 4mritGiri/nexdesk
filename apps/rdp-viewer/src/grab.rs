//! Keyboard capture, so Super, Alt+Tab, etc. go to the *remote* machine instead of the
//! local desktop (like mstsc's "Apply Windows key combinations: on the remote computer").
//!
//! * X11:     XInput2 `XIGrabDevice` on the master keyboard. Key events still arrive through
//!            winit's own connection, so the normal input path keeps working.
//! * Wayland: `zwp_keyboard_shortcuts_inhibit_v1` (supported by GNOME/Mutter, KDE/KWin,
//!            wlroots). GNOME shows a small notice; its "restore shortcuts" key combination
//!            (Super+Escape) always works as an escape hatch.
//! * Other platforms: no-op (Windows/macOS deliver these keys to the app differently).
//!
//! Everything is best-effort: on failure we log a warning and carry on without capture.
use winit::window::Window;

pub struct KeyboardGrab {
    #[cfg(target_os = "linux")]
    inner: Option<linux::Active>,
}

impl KeyboardGrab {
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            inner: None,
        }
    }

    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.inner.is_some()
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    /// Start or stop capturing. Safe to call repeatedly with the same value.
    pub fn set(&mut self, window: &Window, on: bool) {
        #[cfg(target_os = "linux")]
        {
            if on == self.inner.is_some() {
                return;
            }
            if on {
                match linux::Active::start(window) {
                    Ok(a) => self.inner = Some(a),
                    Err(e) => tracing::warn!("keyboard capture unavailable: {e}"),
                }
            } else if let Some(a) = self.inner.take() {
                a.stop();
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (window, on);
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::os::raw::{c_int, c_uchar, c_ulong};

    use winit::raw_window_handle::{
        HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
    };
    use winit::window::Window;

    pub enum Active {
        X11(X11Grab),
        Wayland(WaylandGrab),
    }

    impl Active {
        pub fn start(window: &Window) -> Result<Self, String> {
            let wh = window.window_handle().map_err(|e| e.to_string())?.as_raw();
            let dh = window.display_handle().map_err(|e| e.to_string())?.as_raw();
            match (wh, dh) {
                (RawWindowHandle::Xlib(w), RawDisplayHandle::Xlib(d)) => {
                    let display = d.display.ok_or("no X display")?.as_ptr();
                    X11Grab::start(display.cast(), w.window as c_ulong).map(Active::X11)
                }
                (RawWindowHandle::Wayland(w), RawDisplayHandle::Wayland(d)) => {
                    WaylandGrab::start(d.display.as_ptr(), w.surface.as_ptr()).map(Active::Wayland)
                }
                _ => Err("unsupported windowing system".into()),
            }
        }

        pub fn stop(self) {
            match self {
                Active::X11(g) => g.stop(),
                Active::Wayland(g) => g.stop(),
            }
        }
    }

    // ------------------------------------------------------------------ X11

    pub struct X11Grab {
        xlib: x11_dl::xlib::Xlib,
        xi: x11_dl::xinput2::XInput2,
        display: *mut x11_dl::xlib::Display,
        device: c_int,
    }

    impl X11Grab {
        fn start(display: *mut x11_dl::xlib::Display, window: c_ulong) -> Result<Self, String> {
            let xlib = x11_dl::xlib::Xlib::open().map_err(|e| e.to_string())?;
            let xi = x11_dl::xinput2::XInput2::open().map_err(|e| e.to_string())?;

            // Find the master keyboard (normally id 3, but ask the server).
            let mut count: c_int = 0;
            let infos = unsafe {
                (xi.XIQueryDevice)(display, x11_dl::xinput2::XIAllMasterDevices, &mut count)
            };
            if infos.is_null() {
                return Err("XIQueryDevice failed (no XInput2?)".into());
            }
            let mut device = None;
            for i in 0..count.max(0) as usize {
                let info = unsafe { &*infos.add(i) };
                if info._use == x11_dl::xinput2::XIMasterKeyboard {
                    device = Some(info.deviceid);
                    break;
                }
            }
            unsafe { (xi.XIFreeDeviceInfo)(infos) };
            let device = device.ok_or("no master keyboard found")?;

            // Deliver key press/release to our window as XI2 events (what winit listens to).
            let mut mask: [c_uchar; 4] = [0; 4];
            mask[0] = ((1u32 << x11_dl::xinput2::XI_KeyPress)
                | (1u32 << x11_dl::xinput2::XI_KeyRelease)) as c_uchar;
            let mut event_mask = x11_dl::xinput2::XIEventMask {
                deviceid: device,
                mask_len: mask.len() as c_int,
                mask: mask.as_mut_ptr(),
            };
            let status = unsafe {
                (xi.XIGrabDevice)(
                    display,
                    device,
                    window,
                    0, // CurrentTime
                    0, // no cursor change
                    x11_dl::xinput2::XIGrabModeAsync,
                    x11_dl::xinput2::XIGrabModeAsync,
                    0, // owner_events = False
                    &mut event_mask,
                )
            };
            unsafe { (xlib.XFlush)(display) };
            if status != x11_dl::xinput2::XIGrabSuccess {
                return Err(format!("XIGrabDevice refused (status {status})"));
            }
            Ok(Self {
                xlib,
                xi,
                display,
                device,
            })
        }

        fn stop(self) {
            unsafe {
                (self.xi.XIUngrabDevice)(self.display, self.device, 0);
                (self.xlib.XFlush)(self.display);
            }
        }
    }

    // -------------------------------------------------------------- Wayland

    use wayland_client::backend::{Backend, ObjectId};
    use wayland_client::globals::{registry_queue_init, GlobalListContents};
    use wayland_client::protocol::{wl_registry, wl_seat, wl_surface};
    use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
    use wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::{
        zwp_keyboard_shortcuts_inhibit_manager_v1::{self, ZwpKeyboardShortcutsInhibitManagerV1},
        zwp_keyboard_shortcuts_inhibitor_v1::{self, ZwpKeyboardShortcutsInhibitorV1},
    };

    struct WlState;

    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for WlState {
        fn event(
            _: &mut Self,
            _: &wl_registry::WlRegistry,
            _: wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    impl Dispatch<wl_seat::WlSeat, ()> for WlState {
        fn event(
            _: &mut Self,
            _: &wl_seat::WlSeat,
            _: wl_seat::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    impl Dispatch<ZwpKeyboardShortcutsInhibitManagerV1, ()> for WlState {
        fn event(
            _: &mut Self,
            _: &ZwpKeyboardShortcutsInhibitManagerV1,
            _: zwp_keyboard_shortcuts_inhibit_manager_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    impl Dispatch<ZwpKeyboardShortcutsInhibitorV1, ()> for WlState {
        fn event(
            _: &mut Self,
            _: &ZwpKeyboardShortcutsInhibitorV1,
            event: zwp_keyboard_shortcuts_inhibitor_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            tracing::debug!("shortcuts inhibitor: {event:?}");
        }
    }

    pub struct WaylandGrab {
        conn: Connection,
        queue: EventQueue<WlState>,
        inhibitor: ZwpKeyboardShortcutsInhibitorV1,
    }

    impl WaylandGrab {
        fn start(
            display: *mut std::ffi::c_void,
            surface: *mut std::ffi::c_void,
        ) -> Result<Self, String> {
            // Wrap winit's existing connection (we do not own it, so it is never closed by us).
            let backend = unsafe { Backend::from_foreign_display(display.cast()) };
            let conn = Connection::from_backend(backend);

            let (globals, mut queue) =
                registry_queue_init::<WlState>(&conn).map_err(|e| e.to_string())?;
            let qh = queue.handle();
            let manager: ZwpKeyboardShortcutsInhibitManagerV1 = globals
                .bind(&qh, 1..=1, ())
                .map_err(|_| "compositor lacks keyboard-shortcuts-inhibit".to_string())?;
            let seat: wl_seat::WlSeat = globals
                .bind(&qh, 1..=1, ())
                .map_err(|_| "no wl_seat".to_string())?;

            let id =
                unsafe { ObjectId::from_ptr(wl_surface::WlSurface::interface(), surface.cast()) }
                    .map_err(|e| e.to_string())?;
            let surface = wl_surface::WlSurface::from_id(&conn, id).map_err(|e| e.to_string())?;

            let inhibitor = manager.inhibit_shortcuts(&surface, &seat, &qh, ());
            queue.roundtrip(&mut WlState).map_err(|e| e.to_string())?;
            Ok(Self {
                conn,
                queue,
                inhibitor,
            })
        }

        fn stop(mut self) {
            self.inhibitor.destroy();
            let _ = self.queue.roundtrip(&mut WlState);
            let _ = self.conn.flush();
        }
    }
}
