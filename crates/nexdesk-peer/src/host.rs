//! Agent: share this X11 screen with one authenticated, consenting viewer at a time.
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nexdesk_crypto::{Identity, IdentityPublic, Responder};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};
use x11rb::rust_connection::RustConnection;

use crate::inject::Injector;
use crate::link::{read_frame, write_frame, Reader, Writer, MAX_PRE_AUTH};
use crate::wire::{pack_pixels, Msg};
use crate::PeerError;

const TILE: usize = 64;
const FRAME_TIME: Duration = Duration::from_millis(33);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(45);

/// Who may connect and what they may do.
pub struct Policy {
    /// The viewer sees the screen but its input is ignored.
    pub view_only: bool,
    /// Fingerprints (`SHA256:...`) that are accepted without asking.
    pub allow: Vec<String>,
    /// Asked for every other viewer; `None` means "deny everyone not on the allow list".
    pub prompt: Option<Box<dyn Fn(&IdentityPublic) -> bool + Send + Sync>>,
}

impl Policy {
    fn permits(&self, peer: &IdentityPublic) -> bool {
        let fp = peer.fingerprint_string();
        if self.allow.iter().any(|a| a == &fp) {
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
    w: u16,
    h: u16,
    prev: Vec<u8>,
}

impl Capture {
    fn new() -> Result<Self, PeerError> {
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
        Ok(Self { root: s.root, w: s.width_in_pixels, h: s.height_in_pixels, prev: Vec::new(), conn })
    }

    fn grab(&self) -> Result<Vec<u8>, PeerError> {
        let r = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, self.root, 0, 0, self.w, self.h, !0)
            .map_err(cap)?
            .reply()
            .map_err(cap)?;
        if r.data.len() != self.w as usize * self.h as usize * 4 {
            return Err(cap("unexpected image size"));
        }
        Ok(r.data)
    }

    fn size_changed(&self) -> bool {
        match self.conn.get_geometry(self.root).ok().and_then(|c| c.reply().ok()) {
            Some(g) => g.width != self.w || g.height != self.h,
            None => false,
        }
    }

    /// Tiles that changed since the last call, merged per tile row, as ready-to-send messages.
    fn next_update(&mut self) -> Result<Vec<Msg>, PeerError> {
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
    result.map(|_| fp)
}

fn session_loop(mut rd: Reader, writer: &mut Writer, policy: &Policy) -> Result<(), PeerError> {
    let mut capture = match Capture::new() {
        Ok(c) => c,
        Err(e) => {
            let _ = writer.send(&Msg::Bye("the agent cannot capture its screen".into()));
            return Err(e);
        }
    };
    let injector = if policy.view_only { None } else { Injector::new().ok() };
    writer.send(&Msg::Hello { view_only: policy.view_only || injector.is_none(), width: capture.w, height: capture.h })?;

    let done = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Msg>();
    let reader_done = done.clone();
    let input = std::thread::Builder::new().name("agent-input".into()).spawn(move || {
        loop {
            match rd.recv() {
                Ok(Msg::Ping(n)) => {
                    let _ = tx.send(Msg::Pong(n));
                }
                Ok(m) => {
                    if let Some(inj) = &injector {
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
                writer.send(&m)?;
            }
            for m in capture.next_update()? {
                writer.send(&m)?;
            }
            if last_size_check.elapsed() > Duration::from_secs(1) {
                last_size_check = Instant::now();
                if capture.size_changed() {
                    writer.send(&Msg::Bye("the screen size changed, reconnect".into()))?;
                    break;
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
    let log = Arc::new(log);
    for stream in listener.incoming().flatten() {
        let from = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
        if busy.swap(true, Ordering::SeqCst) {
            log(&format!("refused {from}: a session is already running"));
            continue;
        }
        let (id, policy, busy, log) = (id.clone(), policy.clone(), busy.clone(), log.clone());
        std::thread::spawn(move || {
            log(&format!("connection from {from}"));
            match serve_connection(stream, &id, &policy) {
                Ok(fp) => log(&format!("session with {fp} ended")),
                Err(e) => {
                    log(&format!("connection from {from} failed: {e}"));
                    std::thread::sleep(Duration::from_millis(500)); // slow down guessing
                }
            }
            busy.store(false, Ordering::SeqCst);
        });
    }
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
    fn extract_copies_the_right_pixels() {
        let w = 4;
        let cur: Vec<u8> = (0..4 * 3 * 4).map(|i| i as u8).collect();
        let e = extract(&cur, w, (1, 1, 2, 2));
        assert_eq!(e.len(), 16);
        assert_eq!(&e[..8], &cur[(w + 1) * 4..(w + 1) * 4 + 8]);
    }
}
