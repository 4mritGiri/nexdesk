//! Sharing a Wayland desktop: the desktop's own portals do the work.
//!
//! * `org.freedesktop.portal.ScreenCast` shows the desktop's "share your screen" dialog (the person at
//!   this computer picks what is shared; nothing is captured before that) and hands out a PipeWire stream.
//! * `org.freedesktop.portal.RemoteDesktop` injects pointer and keyboard input into that session.
//!
//! The picture arrives as raw frames from PipeWire and is converted to the same BGRX layout the X11
//! capture produces, so the tile diffing and the wire format are shared.
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use ashpd::desktop::remote_desktop::{
    Axis, DeviceType, KeyState, NotifyKeyboardKeycodeOptions, RemoteDesktop, SelectDevicesOptions,
};
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::PersistMode;
use pipewire as pw;
use pw::spa;

/// Largest picture accepted from the compositor (per side, and in bytes).
const MAX_SIDE: u32 = 16_384;
const MAX_BYTES: usize = 256 * 1024 * 1024;
/// How long the person has to answer the desktop's sharing dialog.
const DIALOG_TIMEOUT: Duration = Duration::from_secs(120);

/// True when this process runs inside a Wayland session.
pub fn is_wayland_session() -> bool {
    std::env::var("XDG_SESSION_TYPE")
        .map(|v| v.eq_ignore_ascii_case("wayland"))
        .unwrap_or(false)
        || (std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("DISPLAY").is_none())
}

/// The latest picture, BGRX, `w * h * 4` bytes.
#[derive(Default)]
struct Frame {
    w: u32,
    h: u32,
    data: Vec<u8>,
    seq: u64,
}

enum Input {
    Motion {
        x: f64,
        y: f64,
    },
    Button {
        code: i32,
        down: bool,
    },
    Wheel {
        dx: i16,
        dy: i16,
    },
    Key {
        code: u16,
        down: bool,
    },
    /// Sent when the share ends, so the portal task wakes up and closes its session.
    Stop,
}

struct Ready {
    node: u32,
    /// Size of the shared area in the compositor's logical coordinates (pointer positions use this).
    logical: (i32, i32),
    fd: OwnedFd,
}

/// A running Wayland sharing session. Dropping it stops the capture and closes the portal session.
pub struct Wl {
    frame: Arc<Mutex<Frame>>,
    input: tokio::sync::mpsc::UnboundedSender<Input>,
    stop: Arc<AtomicBool>,
    quit_pw: Option<pw::channel::Sender<()>>,
    logical: (i32, i32),
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl Wl {
    /// Ask the desktop to share a screen and start receiving it. Blocks while the dialog is open.
    /// `want_input` also asks for mouse and keyboard control (left out for view-only sessions, so the
    /// desktop's dialog only mentions what is really shared).
    pub fn start(want_input: bool) -> Result<Arc<Self>, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let frame = Arc::new(Mutex::new(Frame::default()));
        let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel::<Input>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<Ready, String>>();

        let stop_p = stop.clone();
        let portal = std::thread::Builder::new()
            .name("wl-portal".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("cannot start the portal runtime: {e}")));
                        return;
                    }
                };
                rt.block_on(portal_main(ready_tx, in_rx, stop_p, want_input));
            })
            .map_err(|e| e.to_string())?;

        let ready = match ready_rx.recv_timeout(DIALOG_TIMEOUT + Duration::from_secs(10)) {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                stop.store(true, Ordering::SeqCst);
                let _ = portal.join();
                return Err(e);
            }
            Err(_) => {
                stop.store(true, Ordering::SeqCst);
                return Err("the screen sharing dialog was not answered in time".into());
            }
        };

        let (pw_tx, pw_rx) = pw::channel::channel::<()>();
        let (pw_ready_tx, pw_ready_rx) = mpsc::channel::<Result<(), String>>();
        let frame_pw = frame.clone();
        let (node, fd) = (ready.node, ready.fd);
        let pw_thread = std::thread::Builder::new()
            .name("wl-pipewire".into())
            .spawn(move || run_pipewire(node, fd, frame_pw, pw_rx, pw_ready_tx))
            .map_err(|e| e.to_string())?;
        match pw_ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                stop.store(true, Ordering::SeqCst);
                let _ = pw_tx.send(());
                let _ = pw_thread.join();
                let _ = portal.join();
                return Err(e);
            }
            Err(_) => {
                stop.store(true, Ordering::SeqCst);
                let _ = pw_tx.send(());
                return Err("PipeWire did not start".into());
            }
        }
        Ok(Arc::new(Self {
            frame,
            input: in_tx,
            stop,
            quit_pw: Some(pw_tx),
            logical: ready.logical,
            threads: vec![portal, pw_thread],
        }))
    }

    /// Receive from an existing PipeWire node over `fd` without any portal (used by tests; input is ignored).
    #[doc(hidden)]
    pub fn attach_for_test(fd: OwnedFd, node: u32) -> Result<Arc<Self>, String> {
        let frame = Arc::new(Mutex::new(Frame::default()));
        let (in_tx, _in_rx) = tokio::sync::mpsc::unbounded_channel::<Input>();
        let (pw_tx, pw_rx) = pw::channel::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let f2 = frame.clone();
        let t = std::thread::spawn(move || run_pipewire(node, fd, f2, pw_rx, ready_tx));
        ready_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "PipeWire did not start".to_string())??;
        Ok(Arc::new(Self {
            frame,
            input: in_tx,
            stop: Arc::new(AtomicBool::new(false)),
            quit_pw: Some(pw_tx),
            logical: (0, 0),
            threads: vec![t],
        }))
    }

    /// Wait for the first picture; returns its size.
    pub fn wait_first_frame(&self, timeout: Duration) -> Option<(u32, u32)> {
        let end = Instant::now() + timeout;
        while Instant::now() < end {
            if let Some((w, h, seq)) = self.state() {
                if seq > 0 {
                    return Some((w, h));
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    /// (width, height, picture counter).
    pub fn state(&self) -> Option<(u32, u32, u64)> {
        let f = self.frame.lock().ok()?;
        Some((f.w, f.h, f.seq))
    }

    /// A copy of the latest picture, BGRX.
    pub fn grab(&self) -> Option<(u32, u32, Vec<u8>)> {
        let f = self.frame.lock().ok()?;
        (f.seq > 0 && f.data.len() == f.w as usize * f.h as usize * 4)
            .then(|| (f.w, f.h, f.data.clone()))
    }

    // ---- input; positions are in picture pixels

    pub fn move_to(&self, x: u16, y: u16) {
        let Some((w, h, _)) = self.state().filter(|(w, h, _)| *w > 0 && *h > 0) else {
            return;
        };
        let (lw, lh) = self.logical;
        let (lw, lh) = if lw > 0 && lh > 0 {
            (f64::from(lw), f64::from(lh))
        } else {
            (f64::from(w), f64::from(h))
        };
        let fx = (f64::from(x).min(f64::from(w) - 1.0) + 0.5) * lw / f64::from(w);
        let fy = (f64::from(y).min(f64::from(h) - 1.0) + 0.5) * lh / f64::from(h);
        let _ = self.input.send(Input::Motion { x: fx, y: fy });
    }

    /// 1 = left, 2 = middle, 3 = right.
    pub fn button(&self, button: u8, down: bool) {
        let code = match button {
            1 => 0x110, // BTN_LEFT
            2 => 0x112, // BTN_MIDDLE
            3 => 0x111, // BTN_RIGHT
            _ => return,
        };
        let _ = self.input.send(Input::Button { code, down });
    }

    pub fn wheel(&self, dx: i16, dy: i16) {
        let _ = self.input.send(Input::Wheel { dx, dy });
    }

    /// `code` is a Linux evdev key code.
    pub fn key(&self, code: u16, down: bool) {
        let _ = self.input.send(Input::Key { code, down });
    }
}

impl Drop for Wl {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(q) = self.quit_pw.take() {
            let _ = q.send(());
        }
        let _ = self.input.send(Input::Stop);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

// ---------------------------------------------------------------------------------------- portal

enum Portal {
    Input(RemoteDesktop, ashpd::desktop::Session<RemoteDesktop>),
    View(ashpd::desktop::Session<Screencast>),
}

type Setup = (Portal, u32, (i32, i32), OwnedFd);

/// Screen and input: one RemoteDesktop session that also carries the ScreenCast stream.
async fn setup_with_input() -> Result<Setup, String> {
    let remote = RemoteDesktop::new()
        .await
        .map_err(|e| format!("the desktop has no RemoteDesktop portal ({e})"))?;
    let cast = Screencast::new()
        .await
        .map_err(|e| format!("the desktop has no ScreenCast portal ({e})"))?;
    let session = remote
        .create_session(Default::default())
        .await
        .map_err(|e| format!("cannot create a sharing session ({e})"))?;
    remote
        .select_devices(
            &session,
            SelectDevicesOptions::default()
                .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
                .set_persist_mode(PersistMode::DoNot),
        )
        .await
        .map_err(|e| format!("cannot ask for input devices ({e})"))?;
    cast.select_sources(
        &session,
        SelectSourcesOptions::default()
            .set_cursor_mode(CursorMode::Embedded)
            .set_sources(SourceType::Monitor | SourceType::Window)
            .set_multiple(false)
            .set_persist_mode(PersistMode::DoNot),
    )
    .await
    .map_err(|e| format!("cannot ask for a screen ({e})"))?;
    let started = remote
        .start(&session, None, Default::default())
        .await
        .map_err(|e| format!("the sharing dialog failed ({e})"))?
        .response()
        .map_err(|_| "sharing was declined on this computer".to_string())?;
    let stream = started
        .streams()
        .first()
        .cloned()
        .ok_or("no screen was selected")?;
    if !started.devices().contains(DeviceType::Pointer)
        || !started.devices().contains(DeviceType::Keyboard)
    {
        eprintln!("note: the desktop did not allow mouse/keyboard control for this session");
    }
    let fd = cast
        .open_pipe_wire_remote(&session, Default::default())
        .await
        .map_err(|e| format!("cannot open PipeWire ({e})"))?;
    Ok((
        Portal::Input(remote, session),
        stream.pipe_wire_node_id(),
        stream.size().unwrap_or((0, 0)),
        fd,
    ))
}

/// Screen only (view-only sessions).
async fn setup_view_only() -> Result<Setup, String> {
    let cast = Screencast::new()
        .await
        .map_err(|e| format!("the desktop has no ScreenCast portal ({e})"))?;
    let session = cast
        .create_session(Default::default())
        .await
        .map_err(|e| format!("cannot create a sharing session ({e})"))?;
    cast.select_sources(
        &session,
        SelectSourcesOptions::default()
            .set_cursor_mode(CursorMode::Embedded)
            .set_sources(SourceType::Monitor | SourceType::Window)
            .set_multiple(false)
            .set_persist_mode(PersistMode::DoNot),
    )
    .await
    .map_err(|e| format!("cannot ask for a screen ({e})"))?;
    let started = cast
        .start(&session, None, Default::default())
        .await
        .map_err(|e| format!("the sharing dialog failed ({e})"))?
        .response()
        .map_err(|_| "sharing was declined on this computer".to_string())?;
    let stream = started
        .streams()
        .first()
        .cloned()
        .ok_or("no screen was selected")?;
    let fd = cast
        .open_pipe_wire_remote(&session, Default::default())
        .await
        .map_err(|e| format!("cannot open PipeWire ({e})"))?;
    Ok((
        Portal::View(session),
        stream.pipe_wire_node_id(),
        stream.size().unwrap_or((0, 0)),
        fd,
    ))
}

async fn portal_main(
    ready: mpsc::Sender<Result<Ready, String>>,
    mut input: tokio::sync::mpsc::UnboundedReceiver<Input>,
    stop: Arc<AtomicBool>,
    want_input: bool,
) {
    let setup = tokio::time::timeout(DIALOG_TIMEOUT, async {
        if want_input {
            setup_with_input().await
        } else {
            setup_view_only().await
        }
    })
    .await;
    let (portal, node, logical, fd) = match setup {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            let _ = ready.send(Err(e));
            return;
        }
        Err(_) => {
            let _ = ready.send(Err(
                "the screen sharing dialog was not answered in time".into()
            ));
            return;
        }
    };
    let _ = ready.send(Ok(Ready { node, logical, fd }));

    match &portal {
        Portal::Input(remote, session) => {
            // pointer positions are addressed to the PipeWire stream (its node id)
            while let Some(ev) = input.recv().await {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let r = match ev {
                    Input::Stop => break,
                    Input::Motion { x, y } => {
                        remote
                            .notify_pointer_motion_absolute(session, node, x, y, Default::default())
                            .await
                    }
                    Input::Button { code, down } => {
                        remote
                            .notify_pointer_button(
                                session,
                                code,
                                if down {
                                    KeyState::Pressed
                                } else {
                                    KeyState::Released
                                },
                                Default::default(),
                            )
                            .await
                    }
                    Input::Wheel { dx, dy } => {
                        let steps = |d: i16| -> i32 {
                            (i32::from(d).abs() + 119) / 120 * i32::from(d.signum())
                        };
                        let mut r = Ok(());
                        if dy != 0 {
                            // the viewer sends positive = away from the user (up); the portal counts down as positive
                            r = remote
                                .notify_pointer_axis_discrete(
                                    session,
                                    Axis::Vertical,
                                    -steps(dy),
                                    Default::default(),
                                )
                                .await;
                        }
                        if dx != 0 && r.is_ok() {
                            r = remote
                                .notify_pointer_axis_discrete(
                                    session,
                                    Axis::Horizontal,
                                    steps(dx),
                                    Default::default(),
                                )
                                .await;
                        }
                        r
                    }
                    Input::Key { code, down } => {
                        remote
                            .notify_keyboard_keycode(
                                session,
                                i32::from(code),
                                if down {
                                    KeyState::Pressed
                                } else {
                                    KeyState::Released
                                },
                                NotifyKeyboardKeycodeOptions::default(),
                            )
                            .await
                    }
                };
                if let Err(e) = r {
                    // an input call fails when the person ended the share from the desktop's indicator
                    eprintln!("wayland input stopped: {e}");
                    break;
                }
            }
            let _ = session.close().await;
        }
        Portal::View(session) => {
            // nothing to inject: wait until the session is dropped, then close the share
            while input.recv().await.is_some() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
            }
            let _ = session.close().await;
        }
    }
}

// ------------------------------------------------------------------------------------- pipewire

#[derive(Default)]
struct StreamState {
    format: spa::param::video::VideoInfoRaw,
}

fn run_pipewire(
    node: u32,
    fd: OwnedFd,
    frame: Arc<Mutex<Frame>>,
    quit: pw::channel::Receiver<()>,
    ready: mpsc::Sender<Result<(), String>>,
) {
    let result = (|| -> Result<(), String> {
        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
        let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
        let core = context
            .connect_fd_rc(fd, None)
            .map_err(|e| format!("cannot connect to PipeWire: {e}"))?;
        let ml = mainloop.clone();
        let _quit = quit.attach(mainloop.loop_(), move |_| ml.quit());

        let stream = pw::stream::StreamBox::new(
            &core,
            "nexdesk-capture",
            pw::properties::properties! {
                *pw::keys::MEDIA_TYPE => "Video",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Screen",
            },
        )
        .map_err(|e| e.to_string())?;

        let _listener = stream
            .add_local_listener_with_user_data(StreamState::default())
            .param_changed(|_, st, id, param| {
                let Some(param) = param else { return };
                if id != spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let Ok((mt, ms)) = spa::param::format_utils::parse_format(param) else {
                    return;
                };
                if mt != spa::param::format::MediaType::Video
                    || ms != spa::param::format::MediaSubtype::Raw
                {
                    return;
                }
                let _ = st.format.parse(param);
            })
            .process({
                let frame = frame.clone();
                move |stream, st| {
                    let Some(mut buf) = stream.dequeue_buffer() else {
                        return;
                    };
                    let size = st.format.size();
                    let (w, h) = (size.width, size.height);
                    if w == 0
                        || h == 0
                        || w > MAX_SIDE
                        || h > MAX_SIDE
                        || w as usize * h as usize * 4 > MAX_BYTES
                    {
                        return;
                    }
                    let datas = buf.datas_mut();
                    let Some(d) = datas.first_mut() else { return };
                    let stride = d.chunk().stride().max(0) as usize;
                    let offset = d.chunk().offset() as usize;
                    let chunk_size = d.chunk().size() as usize;
                    let Some(bytes) = d.data() else { return };
                    let fmt = st.format.format();
                    let Some(conv) = convert(
                        bytes, offset, chunk_size, stride, w as usize, h as usize, fmt,
                    ) else {
                        return;
                    };
                    if let Ok(mut f) = frame.lock() {
                        f.w = w;
                        f.h = h;
                        f.data = conv;
                        f.seq += 1;
                    }
                }
            })
            .register()
            .map_err(|e| e.to_string())?;

        let obj = spa::pod::object!(
            spa::utils::SpaTypes::ObjectParamFormat,
            spa::param::ParamType::EnumFormat,
            spa::pod::property!(
                spa::param::format::FormatProperties::MediaType,
                Id,
                spa::param::format::MediaType::Video
            ),
            spa::pod::property!(
                spa::param::format::FormatProperties::MediaSubtype,
                Id,
                spa::param::format::MediaSubtype::Raw
            ),
            spa::pod::property!(
                spa::param::format::FormatProperties::VideoFormat,
                Choice,
                Enum,
                Id,
                spa::param::video::VideoFormat::BGRx,
                spa::param::video::VideoFormat::BGRx,
                spa::param::video::VideoFormat::BGRA,
                spa::param::video::VideoFormat::RGBx,
                spa::param::video::VideoFormat::RGBA,
                spa::param::video::VideoFormat::RGB,
                spa::param::video::VideoFormat::BGR,
            ),
            spa::pod::property!(
                spa::param::format::FormatProperties::VideoSize,
                Choice,
                Range,
                Rectangle,
                spa::utils::Rectangle {
                    width: 1920,
                    height: 1080
                },
                spa::utils::Rectangle {
                    width: 1,
                    height: 1
                },
                spa::utils::Rectangle {
                    width: MAX_SIDE,
                    height: MAX_SIDE
                }
            ),
            spa::pod::property!(
                spa::param::format::FormatProperties::VideoFramerate,
                Choice,
                Range,
                Fraction,
                spa::utils::Fraction { num: 30, denom: 1 },
                spa::utils::Fraction { num: 0, denom: 1 },
                spa::utils::Fraction { num: 120, denom: 1 }
            ),
        );
        let values: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &spa::pod::Value::Object(obj),
        )
        .map_err(|e| format!("{e:?}"))?
        .0
        .into_inner();
        let mut params =
            [spa::pod::Pod::from_bytes(&values).ok_or("bad stream format description")?];
        stream
            .connect(
                spa::utils::Direction::Input,
                Some(node),
                pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
                &mut params,
            )
            .map_err(|e| format!("cannot connect to the screen stream: {e}"))?;
        let _ = ready.send(Ok(()));
        mainloop.run();
        Ok(())
    })();
    if let Err(e) = result {
        let _ = ready.send(Err(e));
    }
}

/// One PipeWire buffer to tightly packed BGRX.
fn convert(
    bytes: &[u8],
    offset: usize,
    size: usize,
    stride: usize,
    w: usize,
    h: usize,
    fmt: spa::param::video::VideoFormat,
) -> Option<Vec<u8>> {
    use spa::param::video::VideoFormat as F;
    let (bpp, order): (usize, [usize; 3]) = match fmt {
        F::BGRx | F::BGRA => (4, [0, 1, 2]),
        F::RGBx | F::RGBA => (4, [2, 1, 0]),
        F::RGB => (3, [2, 1, 0]),
        F::BGR => (3, [0, 1, 2]),
        _ => return None,
    };
    let stride = if stride == 0 { w * bpp } else { stride };
    if stride < w * bpp {
        return None;
    }
    let end = offset.checked_add(stride.checked_mul(h)?)?;
    // the last row may be shorter than a full stride
    let need = end.checked_sub(stride - w * bpp)?;
    if bytes.len() < need || (size != 0 && size < stride * (h - 1) + w * bpp) {
        return None;
    }
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let src = &bytes[offset + y * stride..offset + y * stride + w * bpp];
        let dst = &mut out[y * w * 4..(y + 1) * w * 4];
        if bpp == 4 && order == [0, 1, 2] {
            dst.copy_from_slice(src);
            continue;
        }
        for (s, d) in src.chunks_exact(bpp).zip(dst.chunks_exact_mut(4)) {
            d[0] = s[order[0]];
            d[1] = s[order[1]];
            d[2] = s[order[2]];
            d[3] = 0;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spa::param::video::VideoFormat as F;

    #[test]
    fn converts_common_layouts_and_respects_stride() {
        // 2x2 RGBx with a 12-byte stride (4 bytes of padding per row)
        let src = [
            1, 2, 3, 0, 4, 5, 6, 0, 9, 9, 9, 9, //
            7, 8, 9, 0, 10, 11, 12, 0, 9, 9, 9, 9,
        ];
        let out = convert(&src, 0, 24, 12, 2, 2, F::RGBx).unwrap();
        assert_eq!(out, vec![3, 2, 1, 0, 6, 5, 4, 0, 9, 8, 7, 0, 12, 11, 10, 0]);
        // BGRx is already in the wire layout
        let b = [1, 2, 3, 0, 4, 5, 6, 0];
        assert_eq!(
            convert(&b, 0, 8, 8, 2, 1, F::BGRx).unwrap(),
            vec![1, 2, 3, 0, 4, 5, 6, 0]
        );
        // 3 bytes per pixel
        assert_eq!(
            convert(&[1, 2, 3], 0, 3, 3, 1, 1, F::RGB).unwrap(),
            vec![3, 2, 1, 0]
        );
        // buffers that are too small or have an unknown format are refused, never read out of bounds
        assert!(convert(&b, 0, 8, 8, 2, 2, F::BGRx).is_none());
        assert!(convert(&b, 4, 8, 8, 2, 1, F::BGRx).is_none());
        assert!(convert(&b, 0, 8, 8, 2, 1, F::YUY2).is_none());
        assert!(convert(&b, usize::MAX, 8, 8, 2, 1, F::BGRx).is_none());
    }
}
