//! Agent: share this X11 screen with one authenticated, consenting viewer at a time.
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nexdesk_crypto::{Identity, IdentityPublic, Responder};

use crate::link::{read_frame, write_frame, Reader, Writer, MAX_PRE_AUTH};
use crate::platform::{Capture, Display, Injector};
use crate::wire::Msg;
use crate::PeerError;

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
    /// Asked once per incoming transfer with a one-line summary; `None` refuses all transfers.
    pub files: Option<FilesPrompt>,
    /// Called with every chat line the viewer sends.
    pub on_chat: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    /// Where accepted files are stored (default: the user's Downloads/NexDesk).
    pub download_dir: Option<std::path::PathBuf>,
    outbox: Mutex<Option<mpsc::Sender<Msg>>>,
    recent: Mutex<Option<(String, Instant)>>,
}

pub type FilesPrompt = Arc<dyn Fn(&str) -> bool + Send + Sync>;
const FILES_CONSENT_TIMEOUT: Duration = Duration::from_secs(60);

impl Policy {
    pub fn new(
        view_only: bool,
        clipboard: bool,
        allow: Vec<String>,
        prompt: Option<Box<dyn Fn(&IdentityPublic) -> bool + Send + Sync>>,
    ) -> Self {
        Self {
            view_only,
            clipboard,
            allow,
            prompt,
            reconnect_grace: Duration::from_secs(60),
            files: None,
            on_chat: None,
            download_dir: None,
            outbox: Mutex::new(None),
            recent: Mutex::new(None),
        }
    }

    /// Send a chat line to the connected viewer. False when nobody is connected.
    pub fn send_chat(&self, text: &str) -> bool {
        let t = crate::wire::clean_chat(text);
        if t.is_empty() {
            return false;
        }
        match self.outbox.lock() {
            Ok(o) => o
                .as_ref()
                .map(|tx| tx.send(Msg::Chat(t)).is_ok())
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    fn remember(&self, fp: &str) {
        if let Ok(mut r) = self.recent.lock() {
            *r = Some((fp.to_string(), Instant::now()));
        }
    }

    fn recently_approved(&self, fp: &str) -> bool {
        match self.recent.lock() {
            Ok(r) => {
                matches!(r.as_ref(), Some((f, t)) if f == fp && t.elapsed() < self.reconnect_grace)
            }
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

fn apply_region(inj: &Option<Injector>, capture: &Capture) {
    if let Some(inj) = inj {
        let (x, y, w, h) = capture.region();
        inj.set_region(x, y, w, h);
    }
}

/// Handshake as the responder; the policy decides whether the viewer is allowed in.
fn authenticate(
    mut stream: TcpStream,
    id: &Identity,
    policy: &Policy,
) -> Result<(Reader, Writer, IdentityPublic), PeerError> {
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
    Ok((
        Reader { stream: r, opener },
        Writer { stream, sealer },
        peer,
    ))
}

/// Serve one connection until it ends. Returns the viewer's fingerprint once it was authenticated.
pub fn serve_connection(
    stream: TcpStream,
    id: &Identity,
    policy: &Policy,
) -> Result<String, PeerError> {
    let (reader, mut writer, peer) = authenticate(stream, id, policy)?;
    let fp = peer.fingerprint_string();
    let result = session_loop(reader, &mut writer, policy);
    writer.shutdown();
    policy.remember(&fp); // starts the reconnect grace period
    result.map(|_| fp)
}

fn session_loop(mut rd: Reader, writer: &mut Writer, policy: &Policy) -> Result<(), PeerError> {
    // Wayland: the desktop's own dialog asks the person at this computer what to share (nothing is
    // captured before they answer). X11: the screen is read directly.
    let display = match Display::open(policy.view_only) {
        Ok(d) => d,
        Err(e) => {
            let _ = writer.send(&Msg::Bye(format!("the screen was not shared: {e}")));
            return Err(e);
        }
    };
    let open = |sel: usize| display.capture(sel);
    let mut capture = match open(0) {
        Ok(c) => c,
        Err(e) => {
            let _ = writer.send(&Msg::Bye("the agent cannot capture its screen".into()));
            return Err(e);
        }
    };
    let injector: Arc<Option<Injector>> = Arc::new(if policy.view_only {
        None
    } else {
        display.injector()
    });
    apply_region(&injector, &capture);
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
    if let Ok(mut o) = policy.outbox.lock() {
        *o = Some(tx.clone());
    }
    let receiver = Arc::new(Mutex::new(crate::xfer::Receiver::new(
        policy
            .download_dir
            .clone()
            .or_else(crate::xfer::download_dir)
            .unwrap_or_else(|| std::env::temp_dir().join("NexDesk")),
    )));
    let files_prompt = if policy.view_only {
        None
    } else {
        policy.files.clone()
    };
    let on_chat = policy.on_chat.clone();
    let reader_done = done.clone();
    let input = std::thread::Builder::new()
        .name("agent-input".into())
        .spawn(move || {
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
                    Ok(Msg::Chat(t)) => {
                        if let Some(f) = &on_chat {
                            f(&t);
                        }
                    }
                    Ok(Msg::FilesOffer {
                        batch,
                        count,
                        total,
                        first,
                    }) => {
                        let refused = |tx: &mpsc::Sender<Msg>| {
                            let _ = tx.send(Msg::FilesAnswer {
                                batch,
                                accept: false,
                            });
                        };
                        let Some(prompt) = files_prompt.clone() else {
                            refused(&tx);
                            continue;
                        };
                        let offered = receiver
                            .lock()
                            .map(|mut r| r.offer(batch, count, total))
                            .unwrap_or(Err("busy"));
                        if offered.is_err() {
                            refused(&tx);
                            continue;
                        }
                        // ask without blocking the input loop: pings and input keep flowing
                        let (tx2, receiver2) = (tx.clone(), receiver.clone());
                        let summary = format!("{count}|{total}|{first}");
                        let _ = std::thread::Builder::new()
                            .name("agent-files-consent".into())
                            .spawn(move || {
                                let (rtx, rrx) = mpsc::channel();
                                std::thread::spawn(move || {
                                    let _ = rtx.send(prompt(&summary));
                                });
                                let ok = rrx.recv_timeout(FILES_CONSENT_TIMEOUT).unwrap_or(false);
                                if let Ok(mut r) = receiver2.lock() {
                                    r.answer(batch, ok);
                                }
                                let _ = tx2.send(Msg::FilesAnswer { batch, accept: ok });
                            });
                    }
                    Ok(Msg::FileStart {
                        batch,
                        id,
                        size,
                        path,
                    }) => {
                        let r = receiver
                            .lock()
                            .map(|mut r| r.start(batch, id, size, &path))
                            .unwrap_or(Err("busy".into()));
                        if let Err(e) = r {
                            if let Ok(mut r) = receiver.lock() {
                                r.abort();
                            }
                            let _ = tx.send(Msg::FileAbort { batch, reason: e });
                        }
                    }
                    Ok(Msg::FileChunk { id, data }) => {
                        let r = receiver
                            .lock()
                            .map(|mut r| r.chunk(id, &data))
                            .unwrap_or(Err("busy".into()));
                        match r {
                            Ok(Some(bytes)) => {
                                let _ = tx.send(Msg::FileAck { id, bytes });
                            }
                            Ok(None) => {}
                            Err(e) => {
                                if let Ok(mut r) = receiver.lock() {
                                    r.abort();
                                }
                                let _ = tx.send(Msg::FileAbort {
                                    batch: 0,
                                    reason: e,
                                });
                            }
                        }
                    }
                    Ok(Msg::FileEnd { id }) => {
                        let r = receiver
                            .lock()
                            .map(|mut r| r.end(id))
                            .unwrap_or(Err("busy".into()));
                        match r {
                            Ok((_, size)) => {
                                let _ = tx.send(Msg::FileAck { id, bytes: size });
                            }
                            Err(e) => {
                                if let Ok(mut r) = receiver.lock() {
                                    r.abort();
                                }
                                let _ = tx.send(Msg::FileAbort {
                                    batch: 0,
                                    reason: e,
                                });
                            }
                        }
                    }
                    Ok(Msg::FileAbort { .. }) => {
                        if let Ok(mut r) = receiver.lock() {
                            r.abort();
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
            if let Ok(mut r) = receiver.lock() {
                r.abort(); // connection ended: remove half-written files
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
                        let i = usize::from(i).min(capture.monitor_count() - 1);
                        if i != capture.selected() {
                            capture = open(i)?;
                            apply_region(&injector, &capture);
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
                    capture = open(capture.selected())?;
                    apply_region(&injector, &capture);
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
                    capture = open(capture.selected())?;
                    apply_region(&injector, &capture);
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
    if let Ok(mut o) = policy.outbox.lock() {
        *o = None;
    }
    let _ = input.join();
    match result {
        Err(PeerError::Io(_)) | Err(PeerError::Closed) => Ok(()), // viewer went away
        other => other,
    }
}

/// Accept loop: one viewer at a time, others are dropped immediately.
pub fn run(
    listener: TcpListener,
    id: Arc<Identity>,
    policy: Arc<Policy>,
    log: impl Fn(&str) + Send + Sync + 'static,
) {
    let busy = Arc::new(AtomicBool::new(false));
    run_shared(listener, id, policy, Arc::new(log), busy);
}

pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// Like [`run`], with the "a session is running" flag shared with other ways in (the relay).
pub fn run_shared(
    listener: TcpListener,
    id: Arc<Identity>,
    policy: Arc<Policy>,
    log: Log,
    busy: Arc<AtomicBool>,
) {
    for stream in listener.incoming().flatten() {
        handle_stream(stream, &id, &policy, &busy, &log);
    }
}

/// Serve one incoming connection (direct or through the relay) on its own thread, unless a session is running.
pub fn handle_stream(
    stream: TcpStream,
    id: &Arc<Identity>,
    policy: &Arc<Policy>,
    busy: &Arc<AtomicBool>,
    log: &Log,
) {
    let from = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_default();
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
    fn reconnect_grace_only_covers_the_same_viewer_for_a_short_time() {
        let (a, b) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let p = Policy::new(false, true, vec![], None);
        assert!(!p.permits(a.public()), "nobody is approved at first");
        p.remember(&a.public().fingerprint_string());
        assert!(
            p.permits(a.public()),
            "same identity right after its session"
        );
        assert!(!p.permits(b.public()), "another identity is not covered");
        let mut p = Policy::new(false, true, vec![], None);
        p.reconnect_grace = Duration::ZERO;
        p.remember(&a.public().fingerprint_string());
        assert!(!p.permits(a.public()), "grace 0 always asks");
    }
}
