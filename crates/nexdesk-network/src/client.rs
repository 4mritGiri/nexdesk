//! Agent side (stay registered, hand out incoming connections) and viewer side (connect by ID).
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::proto::{auth_mac, read_ctl, valid_id, write_ctl, Ctl};
use crate::NetError;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HEARTBEAT: Duration = Duration::from_secs(15);
/// The relay echoes heartbeats; silence for this long means the relay (or the path to it) is gone.
const RELAY_SILENCE: Duration = Duration::from_secs(50);

/// The relay access key from `NEXDESK_RELAY_KEY`, if set (the plain `connect` and `register` use it).
pub fn key_from_env() -> Option<Vec<u8>> {
    std::env::var("NEXDESK_RELAY_KEY")
        .ok()
        .map(|k| k.trim().as_bytes().to_vec())
        .filter(|k| !k.is_empty())
}

/// Send the first frame and return the relay's answer. A relay with an access key answers with a challenge
/// first; we prove knowledge of the key and then read the real answer.
fn first(s: &mut TcpStream, key: Option<&[u8]>, m: &Ctl) -> Result<Ctl, NetError> {
    write_ctl(s, m)?;
    match read_ctl(s)? {
        Ctl::Challenge(nonce) => {
            let key = key.ok_or(NetError::Refused(
                "this relay needs an access key (set NEXDESK_RELAY_KEY)",
            ))?;
            write_ctl(s, &Ctl::Auth(auth_mac(key, &nonce)))?;
            match read_ctl(s)? {
                Ctl::Refused => Err(NetError::Refused("the relay did not accept the access key")),
                other => Ok(other),
            }
        }
        other => Ok(other),
    }
}

fn open(relay: &str) -> Result<TcpStream, NetError> {
    let addr = relay
        .to_socket_addrs()?
        .next()
        .ok_or(NetError::Proto("the relay address does not resolve"))?;
    let s = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(CONNECT_TIMEOUT))?;
    s.set_write_timeout(Some(CONNECT_TIMEOUT))?;
    Ok(s)
}

/// Viewer: ask the relay for the computer with this ID. On success the returned stream is connected to the
/// agent (run the end-to-end handshake on it).
pub fn connect(relay: &str, id: &str) -> Result<TcpStream, NetError> {
    connect_with(relay, id, key_from_env().as_deref())
}

pub fn connect_with(relay: &str, id: &str, key: Option<&[u8]>) -> Result<TcpStream, NetError> {
    if !valid_id(id) {
        return Err(NetError::Refused("an ID is nine digits"));
    }
    let mut s = open(relay)?;
    s.set_read_timeout(Some(Duration::from_secs(25)))?;
    match first(&mut s, key, &Ctl::Connect(id.to_string()))? {
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
    register_with(relay, id, key_from_env(), on_stream, status)
}

pub fn register_with(
    relay: String,
    id: String,
    key: Option<Vec<u8>>,
    on_stream: impl Fn(TcpStream) + Send + Sync + 'static,
    status: impl Fn(&str) + Send + Sync + 'static,
) -> Result<Registration, NetError> {
    if !valid_id(&id) {
        return Err(NetError::Refused("an ID is nine digits"));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let (stop2, on_stream, status) = (stop.clone(), Arc::new(on_stream), Arc::new(status));
    let key = Arc::new(key);
    std::thread::Builder::new()
        .name("relay-register".into())
        .spawn(move || {
            let mut delay = 2u64;
            while !stop2.load(Ordering::SeqCst) {
                status("connecting to the relay");
                match session(&relay, &id, key.as_deref(), &stop2, &on_stream, &status) {
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
    key: Option<&[u8]>,
    stop: &Arc<AtomicBool>,
    on_stream: &Arc<impl Fn(TcpStream) + Send + Sync + 'static>,
    status: &Arc<impl Fn(&str) + Send + Sync + 'static>,
) -> Result<(), NetError> {
    let mut ctl = open(relay)?;
    match first(&mut ctl, key, &Ctl::Register(id.to_string()))? {
        Ctl::Registered => {}
        // our own stale entry disappears once the relay notices the old connection died
        Ctl::Taken => {
            return Err(NetError::Refused(
                "this ID is already registered (retrying)",
            ))
        }
        _ => return Err(NetError::Refused("the relay refused the registration")),
    }
    status("registered");
    ctl.set_read_timeout(Some(RELAY_SILENCE))?;
    let alive = Arc::new(AtomicBool::new(true));
    {
        let (mut w, alive, stop) = (ctl.try_clone()?, alive.clone(), stop.clone());
        std::thread::Builder::new()
            .name("relay-heartbeat".into())
            .spawn(move || {
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
                let key = key.map(|k| k.to_vec());
                let _ = std::thread::Builder::new()
                    .name("relay-accept".into())
                    .spawn(move || {
                        let Ok(mut s) = open(&relay) else { return };
                        if !matches!(
                            first(&mut s, key.as_deref(), &Ctl::Accept(token)),
                            Ok(Ctl::Paired)
                        ) {
                            return;
                        }
                        if s.set_read_timeout(None).is_ok() && s.set_write_timeout(None).is_ok() {
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
