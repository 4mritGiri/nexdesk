//! Agent side (stay registered, hand out incoming connections) and viewer side (connect by ID).
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::proto::{read_ctl, valid_id, write_ctl, Ctl};
use crate::NetError;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HEARTBEAT: Duration = Duration::from_secs(15);
/// The relay echoes heartbeats; silence for this long means the relay (or the path to it) is gone.
const RELAY_SILENCE: Duration = Duration::from_secs(50);

fn open(relay: &str) -> Result<TcpStream, NetError> {
    let addr = relay.to_socket_addrs()?.next().ok_or(NetError::Proto("the relay address does not resolve"))?;
    let s = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(CONNECT_TIMEOUT))?;
    s.set_write_timeout(Some(CONNECT_TIMEOUT))?;
    Ok(s)
}

/// Viewer: ask the relay for the computer with this ID. On success the returned stream is connected to the
/// agent (run the end-to-end handshake on it).
pub fn connect(relay: &str, id: &str) -> Result<TcpStream, NetError> {
    if !valid_id(id) {
        return Err(NetError::Refused("an ID is nine digits"));
    }
    let mut s = open(relay)?;
    write_ctl(&mut s, &Ctl::Connect(id.to_string()))?;
    s.set_read_timeout(Some(Duration::from_secs(25)))?;
    match read_ctl(&mut s)? {
        Ctl::Paired => {
            s.set_read_timeout(None)?;
            s.set_write_timeout(None)?;
            Ok(s)
        }
        Ctl::NotFound => Err(NetError::Refused("no computer with that ID is online")),
        _ => Err(NetError::Refused("the relay refused the request")),
    }
}

/// Keeps an agent registered; dropping it unregisters (the control connection closes within a second).
pub struct Registration {
    stop: Arc<AtomicBool>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn nap(stop: &AtomicBool, d: Duration) {
    let end = std::time::Instant::now() + d;
    while !stop.load(Ordering::SeqCst) && std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Register under `id`, reconnect forever with backoff, and give every viewer's connection to `on_stream`.
/// `status` gets short texts such as "registered" for the user interface.
pub fn register(
    relay: String,
    id: String,
    on_stream: impl Fn(TcpStream) + Send + Sync + 'static,
    status: impl Fn(&str) + Send + Sync + 'static,
) -> Result<Registration, NetError> {
    if !valid_id(&id) {
        return Err(NetError::Refused("an ID is nine digits"));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let (stop2, on_stream, status) = (stop.clone(), Arc::new(on_stream), Arc::new(status));
    std::thread::Builder::new().name("relay-register".into()).spawn(move || {
        let mut delay = 2u64;
        while !stop2.load(Ordering::SeqCst) {
            status("connecting to the relay");
            match session(&relay, &id, &stop2, &on_stream, &status) {
                Ok(()) => delay = 2,
                Err(e) => {
                    status(&format!("relay: {e}"));
                    nap(&stop2, Duration::from_secs(delay));
                    delay = (delay * 2).min(30);
                }
            }
            nap(&stop2, Duration::from_secs(1));
        }
    })?;
    Ok(Registration { stop })
}

fn session(
    relay: &str,
    id: &str,
    stop: &Arc<AtomicBool>,
    on_stream: &Arc<impl Fn(TcpStream) + Send + Sync + 'static>,
    status: &Arc<impl Fn(&str) + Send + Sync + 'static>,
) -> Result<(), NetError> {
    let mut ctl = open(relay)?;
    write_ctl(&mut ctl, &Ctl::Register(id.to_string()))?;
    match read_ctl(&mut ctl)? {
        Ctl::Registered => {}
        // our own stale entry disappears once the relay notices the old connection died
        Ctl::Taken => return Err(NetError::Refused("this ID is already registered (retrying)")),
        _ => return Err(NetError::Refused("the relay refused the registration")),
    }
    status("registered");
    ctl.set_read_timeout(Some(RELAY_SILENCE))?;
    let alive = Arc::new(AtomicBool::new(true));
    {
        let (mut w, alive, stop) = (ctl.try_clone()?, alive.clone(), stop.clone());
        std::thread::Builder::new().name("relay-heartbeat".into()).spawn(move || {
            while alive.load(Ordering::SeqCst) && !stop.load(Ordering::SeqCst) {
                if write_ctl(&mut w, &Ctl::Heartbeat).is_err() {
                    break;
                }
                nap(&stop, HEARTBEAT);
            }
            let _ = w.shutdown(std::net::Shutdown::Both);
        })?;
    }
    let result = loop {
        if stop.load(Ordering::SeqCst) {
            break Ok(());
        }
        match read_ctl(&mut ctl) {
            Ok(Ctl::Heartbeat) => {}
            Ok(Ctl::Incoming(token)) => {
                let (relay, on_stream) = (relay.to_string(), on_stream.clone());
                let _ = std::thread::Builder::new().name("relay-accept".into()).spawn(move || {
                    let Ok(mut s) = open(&relay) else { return };
                    if write_ctl(&mut s, &Ctl::Accept(token)).is_err() {
                        return;
                    }
                    if matches!(read_ctl(&mut s), Ok(Ctl::Paired)) && s.set_read_timeout(None).is_ok() && s.set_write_timeout(None).is_ok() {
                        on_stream(s);
                    }
                });
            }
            Ok(_) => break Err(NetError::Proto("unexpected frame from the relay")),
            Err(e) => break Err(e),
        }
    };
    alive.store(false, Ordering::SeqCst);
    let _ = ctl.shutdown(std::net::Shutdown::Both);
    result
}
