//! Agent: share this X11 screen with one authenticated, consenting viewer at a time.
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nexdesk_crypto::{Identity, IdentityPublic, Responder};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};
use x11rb::rust_connection::RustConnection;

use crate::inject::Injector;
use crate::link::{read_frame, write_frame, Reader, Writer, MAX_PRE_AUTH};
use crate::wire::{pack_pixels, Msg, Rect, MAX_MONITORS};
use crate::PeerError;

const TILE: usize = 64;
const FRAME_TIME: Duration = Duration::from_millis(33);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(45);

/// Who may connect and what they may do.
pub struct Policy {
    /// The viewer sees the screen but its input is ignored.
    pub view_only: bool,
    /// Share text clipboard both ways (ignored for view-only sessions).
    pub clipboard: bool,
    /// Fingerprints (`SHA256:...`) that are accepted without asking.
    pub allow: Vec<String>,
    /// Asked for every other viewer; `None` means "deny everyone not on the allow list".
    pub prompt: Option<Box<dyn Fn(&IdentityPublic) -> bool + Send + Sync>>,
    /// A viewer approved a moment ago may reconnect without being asked again for this long
    /// (its identity is re-verified by the handshake). `Duration::ZERO` always asks.
    pub reconnect_grace: Duration,
    recent: Mutex<Option<(String, Instant)>>,
}

impl Policy {
    pub fn new(
        view_only: bool,
        clipboard: bool,
        allow: Vec<String>,
        prompt: Option<Box<dyn Fn(&IdentityPublic) -> bool + Send + Sync>>,
    ) -> Self {
        Self { view_only, clipboard, allow, prompt, reconnect_grace: Duration::from_secs(60), recent: Mutex::new(None) }
    }

    fn remember(&self, fp: &str) {
        if let Ok(mut r) = self.recent.lock() {
            *r = Some((fp.to_string(), Instant::now()));
        }
    }

    fn recently_approved(&self, fp: &str) -> bool {
        match self.recent.lock() {
            Ok(r) => matches!(r.as_ref(), Some((f, t)) if f == fp && t.elapsed() < self.reconnect_grace),
            Err(_) => false,
        }
    }

    fn permits(&self, peer: &IdentityPublic) -> bool {
        let fp = peer.fingerprint_string();
        if self.allow.iter().any(|a| a == &fp) || self.recently_approved(&fp) {
            return true;
        }
        self.prompt.as_ref().map(|p| p(peer)).unwrap_or(false)
    }
}

fn cap(e: impl std::fmt::Display) -> PeerError {
    PeerError::Capture(e.to_string())
}

struct Capture {
    conn: RustConnection,
    root: u32,
    /// The shared area (one monitor, or the whole screen) in root coordinates.
    ox: i16,
    oy: i16,
    w: u16,
    h: u16,
    /// Whole X screen size and monitor list when this capture was set up.
    root_size: (u16, u16),
    monitors: Vec<Rect>,
    selected: usize,
    prev: Vec<u8>,
    xfixes: bool,
    cursor_serial: u32,
    damage: Option<u32>,
    last_grab: Instant,
}

/// Monitors from RandR; the whole screen when RandR is missing or lists nothing usable.
fn list_monitors(conn: &RustConnection, root: u32, root_w: u16, root_h: u16) -> Vec<Rect> {
    use x11rb::protocol::randr::ConnectionExt as _;
    let mut out: Vec<Rect> = conn
        .randr_get_monitors(root, true)
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| {
            r.monitors
                .iter()
                .filter(|m| m.width > 0 && m.height > 0)
                .map(|m| Rect { x: m.x, y: m.y, w: m.width, h: m.height })
                // keep only monitors inside the screen, so a grab can never fail on bounds
                .filter(|m| m.x >= 0 && m.y >= 0 && m.x as u32 + m.w as u32 <= root_w as u32 && m.y as u32 + m.h as u32 <= root_h as u32)
                .collect()
        })
        .unwrap_or_default();
    out.truncate(MAX_MONITORS);
    if out.is_empty() {
        out.push(Rect { x: 0, y: 0, w: root_w, h: root_h });
    }
    out
}

impl Capture {
    fn new(selected: usize) -> Result<Self, PeerError> {
        let (conn, n) = x11rb::connect(None).map_err(cap)?;
        let setup = conn.setup();
        let s = &setup.roots[n];
        let fmt = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == s.root_depth)
            .ok_or_else(|| cap("no pixmap format for the root depth"))?;
        if fmt.bits_per_pixel != 32 {
            return Err(cap(format!("unsupported screen format ({} bits per pixel)", fmt.bits_per_pixel)));
        }
        let root = s.root;
        let (root_w, root_h) = (s.width_in_pixels, s.height_in_pixels);
        let xfixes = x11rb::protocol::xfixes::query_version(&conn, 5, 0).ok().and_then(|c| c.reply().ok()).is_some();
        let monitors = list_monitors(&conn, root, root_w, root_h);
        let selected = selected.min(monitors.len() - 1);
        let m = monitors[selected];
        // XDamage tells us when anything on screen changed, so an idle screen costs nothing
        let damage = (|| {
            use x11rb::protocol::damage::{ConnectionExt as _, ReportLevel};
            conn.damage_query_version(1, 1).ok()?.reply().ok()?;
            let id = conn.generate_id().ok()?;
            conn.damage_create(id, root, ReportLevel::NON_EMPTY).ok()?.check().ok()?;
            Some(id)
        })();
        Ok(Self {
            conn,
            root,
            ox: m.x,
            oy: m.y,
            w: m.w,
            h: m.h,
            root_size: (root_w, root_h),
            monitors,
            selected,
            prev: Vec::new(),
            xfixes,
            cursor_serial: 0,
            damage,
            last_grab: Instant::now(),
        })
    }

    fn grab(&self) -> Result<Vec<u8>, PeerError> {
        let r = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, self.root, self.ox, self.oy, self.w, self.h, !0)
            .map_err(cap)?
            .reply()
            .map_err(cap)?;
        if r.data.len() != self.w as usize * self.h as usize * 4 {
            return Err(cap("unexpected image size"));
        }
        Ok(r.data)
    }

    /// A new pointer image if it changed since the last call.
    fn cursor(&mut self) -> Option<Msg> {
        if !self.xfixes {
            return None;
        }
        let r = x11rb::protocol::xfixes::get_cursor_image(&self.conn).ok()?.reply().ok()?;
        if r.cursor_serial == self.cursor_serial {
            return None;
        }
        self.cursor_serial = r.cursor_serial;
        let (w, h) = (r.width, r.height);
        if w == 0 || h == 0 || w > crate::wire::MAX_CURSOR || h > crate::wire::MAX_CURSOR || r.cursor_image.len() != w as usize * h as usize {
            return None;
        }
        let bytes: Vec<u8> = r.cursor_image.iter().flat_map(|p| p.to_le_bytes()).collect();
        Some(Msg::Cursor { hot_x: r.xhot.min(w - 1), hot_y: r.yhot.min(h - 1), w, h, lz4: pack_pixels(&bytes) })
    }

    /// The screen size or the monitor layout is not what we set up with.
    fn layout_changed(&self) -> bool {
        match self.conn.get_geometry(self.root).ok().and_then(|c| c.reply().ok()) {
            Some(g) => (g.width, g.height) != self.root_size || list_monitors(&self.conn, self.root, g.width, g.height) != self.monitors,
            None => false,
        }
    }

    /// Messages that tell the viewer what it is looking at.
    fn announce(&self, view_only: bool) -> [Msg; 2] {
        [
            Msg::Hello { view_only, width: self.w, height: self.h },
            Msg::Monitors { current: self.selected as u8, rects: self.monitors.clone() },
        ]
    }

    /// True when something on the screen changed since the last grab (always true without XDamage,
    /// and at least once a second as a safety net).
    fn needs_grab(&mut self) -> bool {
        let Some(d) = self.damage else { return true };
        let mut dirty = self.prev.is_empty() || self.last_grab.elapsed() >= Duration::from_secs(1);
        while let Ok(Some(ev)) = self.conn.poll_for_event() {
            if matches!(ev, x11rb::protocol::Event::DamageNotify(_)) {
                dirty = true;
            }
        }
        if dirty {
            use x11rb::protocol::damage::ConnectionExt as _;
            // re-arm: the next change produces a new notification
            let _ = self.conn.damage_subtract(d, 0u32, 0u32);
            let _ = self.conn.flush();
        }
        dirty
    }

    /// Tiles that changed since the last call, merged per tile row, as ready-to-send messages.
    fn next_update(&mut self) -> Result<Vec<Msg>, PeerError> {
        if !self.needs_grab() {
            return Ok(Vec::new());
        }
        self.last_grab = Instant::now();
        let cur = self.grab()?;
        let rects = dirty_rects(&self.prev, &cur, self.w as usize, self.h as usize);
        let msgs = rects
            .into_iter()
            .map(|(x, y, w, h)| Msg::Tile {
                x: x as u16,
                y: y as u16,
                w: w as u16,
                h: h as u16,
                lz4: pack_pixels(&extract(&cur, self.w as usize, (x, y, w, h))),
            })
            .collect();
        self.prev = cur;
        Ok(msgs)
    }
}

/// Changed regions between two BGRX screens: per 64-pixel tile row, runs of adjacent changed tiles.
/// With an empty or differently sized `prev` everything is changed.
pub fn dirty_rects(prev: &[u8], cur: &[u8], w: usize, h: usize) -> Vec<(usize, usize, usize, usize)> {
    let full = prev.len() != cur.len();
    let mut out = Vec::new();
    let stride = w * 4;
    for ty in (0..h).step_by(TILE) {
        let th = TILE.min(h - ty);
        let mut run: Option<usize> = None;
        let flush = |start: Option<usize>, end_x: usize, out: &mut Vec<_>| {
            if let Some(sx) = start {
                out.push((sx, ty, end_x - sx, th));
            }
        };
        for tx in (0..w).step_by(TILE) {
            let tw = TILE.min(w - tx);
            let changed = full
                || (0..th).any(|r| {
                    let o = (ty + r) * stride + tx * 4;
                    prev[o..o + tw * 4] != cur[o..o + tw * 4]
                });
            match (changed, run) {
                (true, None) => run = Some(tx),
                (false, Some(_)) => {
                    flush(run.take(), tx, &mut out);
                }
                _ => {}
            }
        }
        flush(run, w, &mut out);
    }
    out
}

fn extract(cur: &[u8], w: usize, (x, y, rw, rh): (usize, usize, usize, usize)) -> Vec<u8> {
    let mut out = Vec::with_capacity(rw * rh * 4);
    for r in 0..rh {
        let o = ((y + r) * w + x) * 4;
        out.extend_from_slice(&cur[o..o + rw * 4]);
    }
    out
}

/// Handshake as the responder; the policy decides whether the viewer is allowed in.
fn authenticate(mut stream: TcpStream, id: &Identity, policy: &Policy) -> Result<(Reader, Writer, IdentityPublic), PeerError> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let msg1 = read_frame(&mut stream, MAX_PRE_AUTH)?;
    let (waiting, msg2) = Responder::respond(id, &msg1)?;
    write_frame(&mut stream, &msg2)?;
    let msg3 = read_frame(&mut stream, MAX_PRE_AUTH)?;
    // The consent prompt may take a while: lift the handshake timeout for it.
    stream.set_read_timeout(None)?;
    let (session, peer) = waiting.finish(&msg3, |p| policy.permits(p))?;
    stream.set_read_timeout(Some(IDLE_TIMEOUT))?;
    stream.set_write_timeout(None)?;
    let (sealer, opener) = session.split();
    let r = stream.try_clone()?;
    Ok((Reader { stream: r, opener }, Writer { stream, sealer }, peer))
}

/// Serve one connection until it ends. Returns the viewer's fingerprint once it was authenticated.
pub fn serve_connection(stream: TcpStream, id: &Identity, policy: &Policy) -> Result<String, PeerError> {
    let (reader, mut writer, peer) = authenticate(stream, id, policy)?;
    let fp = peer.fingerprint_string();
    let result = session_loop(reader, &mut writer, policy);
    writer.shutdown();
    policy.remember(&fp); // starts the reconnect grace period
    result.map(|_| fp)
}

fn session_loop(mut rd: Reader, writer: &mut Writer, policy: &Policy) -> Result<(), PeerError> {
    let mut capture = match Capture::new(0) {
        Ok(c) => c,
        Err(e) => {
            let _ = writer.send(&Msg::Bye("the agent cannot capture its screen".into()));
            return Err(e);
        }
    };
    let injector: Arc<Option<Injector>> = Arc::new(if policy.view_only { None } else { Injector::new().ok() });
    let injector_input = injector.clone();
    let view_only = policy.view_only || injector.is_none();
    for m in capture.announce(view_only) {
        writer.send(&m)?;
    }

    let done = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Msg>();
    let clip = if policy.clipboard && !policy.view_only {
        match crate::clip::ClipSync::start(tx.clone()) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("clipboard sharing unavailable: {e}");
                None
            }
        }
    } else {
        None
    };
    let reader_done = done.clone();
    let input = std::thread::Builder::new().name("agent-input".into()).spawn(move || {
        loop {
            match rd.recv() {
                Ok(Msg::Ping(n)) => {
                    let _ = tx.send(Msg::Pong(n));
                }
                Ok(Msg::SelectMonitor(i)) => {
                    let _ = tx.send(Msg::SelectMonitor(i)); // handled by the capture loop
                }
                Ok(Msg::Clip(t)) => {
                    if let Some(c) = &clip {
                        c.set_remote(t);
                    }
                }
                Ok(m) => {
                    if let Some(inj) = &*injector_input {
                        let _ = match m {
                            Msg::MouseMove { x, y } => inj.move_to(x, y),
                            Msg::MouseButton { button, down } => inj.button(button, down),
                            Msg::Wheel { dx, dy } => inj.wheel(dx, dy),
                            Msg::Key { code, down } => inj.key(code, down),
                            _ => Ok(()),
                        };
                    }
                }
                Err(_) => break,
            }
            if reader_done.load(Ordering::Relaxed) {
                break;
            }
        }
        reader_done.store(true, Ordering::Relaxed);
    })?;

    let mut last_size_check = Instant::now();
    let result = (|| -> Result<(), PeerError> {
        while !done.load(Ordering::Relaxed) {
            let t0 = Instant::now();
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::SelectMonitor(i) => {
                        let i = usize::from(i).min(capture.monitors.len() - 1);
                        if i != capture.selected {
                            capture = Capture::new(i)?;
                            if let Some(inj) = &*injector {
                                inj.set_region(capture.ox, capture.oy, capture.w, capture.h);
                            }
                            for m in capture.announce(view_only) {
                                writer.send(&m)?;
                            }
                        }
                    }
                    m => writer.send(&m)?,
                }
            }
            if let Some(c) = capture.cursor() {
                writer.send(&c)?;
            }
            match capture.next_update() {
                Ok(msgs) => {
                    for m in msgs {
                        writer.send(&m)?;
                    }
                }
                // a grab that fails right after a size change is not fatal: announce the new size
                Err(_) if capture.layout_changed() => {
                    capture = Capture::new(capture.selected)?;
                    if let Some(inj) = &*injector {
                        inj.set_region(capture.ox, capture.oy, capture.w, capture.h);
                    }
                    for m in capture.announce(view_only) {
                        writer.send(&m)?;
                    }
                }
                Err(e) => return Err(e),
            }
            if last_size_check.elapsed() > Duration::from_secs(1) {
                last_size_check = Instant::now();
                if capture.layout_changed() {
                    // announce the new size or monitors; the viewer starts a fresh picture
                    capture = Capture::new(capture.selected)?;
                    if let Some(inj) = &*injector {
                        inj.set_region(capture.ox, capture.oy, capture.w, capture.h);
                    }
                    for m in capture.announce(view_only) {
                        writer.send(&m)?;
                    }
                }
            }
            if let Some(rest) = FRAME_TIME.checked_sub(t0.elapsed()) {
                std::thread::sleep(rest);
            }
        }
        Ok(())
    })();
    done.store(true, Ordering::Relaxed);
    writer.shutdown();
    let _ = input.join();
    match result {
        Err(PeerError::Io(_)) | Err(PeerError::Closed) => Ok(()), // viewer went away
        other => other,
    }
}

/// Accept loop: one viewer at a time, others are dropped immediately.
pub fn run(listener: TcpListener, id: Arc<Identity>, policy: Arc<Policy>, log: impl Fn(&str) + Send + Sync + 'static) {
    let busy = Arc::new(AtomicBool::new(false));
    run_shared(listener, id, policy, Arc::new(log), busy);
}

pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// Like [`run`], with the "a session is running" flag shared with other ways in (the relay).
pub fn run_shared(listener: TcpListener, id: Arc<Identity>, policy: Arc<Policy>, log: Log, busy: Arc<AtomicBool>) {
    for stream in listener.incoming().flatten() {
        handle_stream(stream, &id, &policy, &busy, &log);
    }
}

/// Serve one incoming connection (direct or through the relay) on its own thread, unless a session is running.
pub fn handle_stream(stream: TcpStream, id: &Arc<Identity>, policy: &Arc<Policy>, busy: &Arc<AtomicBool>, log: &Log) {
    let from = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
    if busy.swap(true, Ordering::SeqCst) {
        log(&format!("refused {from}: a session is already running"));
        return;
    }
    let (id, policy, busy, log) = (id.clone(), policy.clone(), busy.clone(), log.clone());
    std::thread::spawn(move || {
        log(&format!("connection from {from}"));
        match serve_connection(stream, &id, &policy) {
            Ok(fp) => log(&format!("session with {fp} ended")),
            Err(e) => {
                log(&format!("connection from {from} failed: {e}"));
                // free the slot first: a viewer that only checked our fingerprint (probe) may
                // reconnect immediately; the pause below only slows this one thread down
                busy.store(false, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(500));
                return;
            }
        }
        busy.store(false, Ordering::SeqCst);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_rects_find_only_what_changed() {
        let (w, h) = (200usize, 130usize);
        let a = vec![0u8; w * h * 4];
        // first frame (no previous): everything, merged into one run per tile row
        let all = dirty_rects(&[], &a, w, h);
        assert_eq!(all.len(), 3, "rows: 64 + 64 + 2 px");
        assert!(all.iter().all(|r| r.0 == 0 && r.2 == w));
        assert_eq!(all[2], (0, 128, w, 2));
        // identical frames: nothing
        assert!(dirty_rects(&a, &a, w, h).is_empty());
        // change one pixel at (70, 10): tile column 1, tile row 0
        let mut b = a.clone();
        b[(10 * w + 70) * 4] = 9;
        assert_eq!(dirty_rects(&a, &b, w, h), vec![(64, 0, 64, 64)]);
        // change pixels in tile columns 0 and 2 of row 1: two separate runs
        let mut c = a.clone();
        c[(70 * w + 3) * 4] = 1;
        c[(70 * w + 140) * 4] = 1;
        assert_eq!(dirty_rects(&a, &c, w, h), vec![(0, 64, 64, 64), (128, 64, 64, 64)]);
        // last partial column / row
        let mut d = a.clone();
        d[(129 * w + 199) * 4 + 1] = 5;
        assert_eq!(dirty_rects(&a, &d, w, h), vec![(192, 128, 8, 2)]);
    }

    #[test]
    fn reconnect_grace_only_covers_the_same_viewer_for_a_short_time() {
        let (a, b) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let p = Policy::new(false, true, vec![], None);
        assert!(!p.permits(a.public()), "nobody is approved at first");
        p.remember(&a.public().fingerprint_string());
        assert!(p.permits(a.public()), "same identity right after its session");
        assert!(!p.permits(b.public()), "another identity is not covered");
        let mut p = Policy::new(false, true, vec![], None);
        p.reconnect_grace = Duration::ZERO;
        p.remember(&a.public().fingerprint_string());
        assert!(!p.permits(a.public()), "grace 0 always asks");
    }

    #[test]
    fn extract_copies_the_right_pixels() {
        let w = 4;
        let cur: Vec<u8> = (0..4 * 3 * 4).map(|i| i as u8).collect();
        let e = extract(&cur, w, (1, 1, 2, 2));
        assert_eq!(e.len(), 16);
        assert_eq!(&e[..8], &cur[(w + 1) * 4..(w + 1) * 4 + 8]);
    }
}
