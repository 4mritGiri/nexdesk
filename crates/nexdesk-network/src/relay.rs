//! The relay server: registry of agents by ID, pairing of viewers with agents, byte bridging.
//! It never looks into the bridged bytes and never logs them; it logs addresses, IDs and counts only.
use std::collections::HashMap;
use std::net::{IpAddr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::proto::{random_token, read_ctl, write_ctl, Ctl, Token};
use crate::NetError;

#[derive(Debug, Clone)]
pub struct Limits {
    /// Connections open at the same time (control, waiting viewers and bridges).
    pub max_connections: usize,
    /// Connections open at the same time from one address.
    pub per_ip: usize,
    /// How long a viewer waits for the agent's second connection.
    pub pair_timeout: Duration,
    /// A bridge with no data in either direction for this long is closed (viewers ping every 10 s).
    pub bridge_idle: Duration,
    /// An agent control connection without a heartbeat for this long is dropped.
    pub agent_silence: Duration,
    /// Viewers waiting for the same agent at the same time.
    pub pending_per_agent: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_connections: 2000,
            per_ip: 30,
            pair_timeout: Duration::from_secs(15),
            bridge_idle: Duration::from_secs(120),
            agent_silence: Duration::from_secs(45),
            pending_per_agent: 2,
        }
    }
}

struct AgentEntry {
    tx: Sender<Ctl>,
    gen: u64,
    pending: Arc<AtomicUsize>,
}

pub struct Relay {
    limits: Limits,
    agents: Mutex<HashMap<String, AgentEntry>>,
    waiting: Mutex<HashMap<Token, Sender<TcpStream>>>,
    per_ip: Mutex<HashMap<IpAddr, usize>>,
    total: AtomicUsize,
    next_gen: AtomicU64,
    log: Box<dyn Fn(&str) + Send + Sync>,
}

/// Counts one connection against the limits until dropped.
struct Slot {
    relay: Arc<Relay>,
    ip: IpAddr,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.relay.total.fetch_sub(1, Ordering::SeqCst);
        if let Ok(mut m) = self.relay.per_ip.lock() {
            if let Some(n) = m.get_mut(&self.ip) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    m.remove(&self.ip);
                }
            }
        }
    }
}

impl Relay {
    pub fn new(limits: Limits, log: impl Fn(&str) + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            limits,
            agents: Mutex::new(HashMap::new()),
            waiting: Mutex::new(HashMap::new()),
            per_ip: Mutex::new(HashMap::new()),
            total: AtomicUsize::new(0),
            next_gen: AtomicU64::new(1),
            log: Box::new(log),
        })
    }

    fn slot(self: &Arc<Self>, ip: IpAddr) -> Option<Slot> {
        if self.total.fetch_add(1, Ordering::SeqCst) >= self.limits.max_connections {
            self.total.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        let mut m = self.per_ip.lock().ok()?;
        let n = m.entry(ip).or_insert(0);
        if *n >= self.limits.per_ip {
            drop(m);
            self.total.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        *n += 1;
        Some(Slot { relay: self.clone(), ip })
    }

    /// Accept connections until the listener fails.
    pub fn serve(self: &Arc<Self>, listener: TcpListener) {
        for stream in listener.incoming().flatten() {
            let Ok(peer) = stream.peer_addr() else { continue };
            let Some(slot) = self.slot(peer.ip()) else {
                let mut s = stream;
                let _ = write_ctl(&mut s, &Ctl::Refused);
                continue;
            };
            let me = self.clone();
            let spawned = std::thread::Builder::new().name("relay-conn".into()).spawn(move || {
                let _slot = slot;
                if let Err(e) = me.handle(stream, peer.ip()) {
                    (me.log)(&format!("{} closed: {e}", peer.ip()));
                }
            });
            if spawned.is_err() {
                (self.log)("cannot start a thread, dropping a connection");
            }
        }
    }

    fn handle(self: &Arc<Self>, mut s: TcpStream, ip: IpAddr) -> Result<(), NetError> {
        s.set_nodelay(true)?;
        s.set_read_timeout(Some(Duration::from_secs(10)))?;
        s.set_write_timeout(Some(Duration::from_secs(10)))?;
        match read_ctl(&mut s)? {
            Ctl::Register(id) => self.agent(s, id, ip),
            Ctl::Connect(id) => self.viewer(s, id, ip),
            Ctl::Accept(token) => {
                let tx = self.waiting.lock().map_err(|_| NetError::Proto("lock"))?.remove(&token);
                match tx {
                    Some(tx) => {
                        // the viewer's thread now owns this connection
                        let _ = tx.send(s);
                        Ok(())
                    }
                    None => write_ctl(&mut s, &Ctl::Refused),
                }
            }
            _ => write_ctl(&mut s, &Ctl::Refused),
        }
    }

    fn agent(self: &Arc<Self>, mut s: TcpStream, id: String, ip: IpAddr) -> Result<(), NetError> {
        let (tx, rx) = channel::<Ctl>();
        let gen = self.next_gen.fetch_add(1, Ordering::SeqCst);
        let pending = Arc::new(AtomicUsize::new(0));
        {
            let mut agents = self.agents.lock().map_err(|_| NetError::Proto("lock"))?;
            if agents.contains_key(&id) {
                return write_ctl(&mut s, &Ctl::Taken);
            }
            agents.insert(id.clone(), AgentEntry { tx: tx.clone(), gen, pending });
        }
        (self.log)(&format!("agent {id} registered from {ip}"));
        let alive = Arc::new(AtomicBool::new(true));
        let result = (|| -> Result<(), NetError> {
            write_ctl(&mut s, &Ctl::Registered)?;
            let mut w = s.try_clone()?;
            let alive_w = alive.clone();
            std::thread::Builder::new().name("relay-agent-writer".into()).spawn(move || {
                while alive_w.load(Ordering::SeqCst) {
                    match rx.recv_timeout(Duration::from_secs(1)) {
                        Ok(m) => {
                            if write_ctl(&mut w, &m).is_err() {
                                break;
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                let _ = w.shutdown(Shutdown::Both);
            })?;
            s.set_read_timeout(Some(self.limits.agent_silence))?;
            loop {
                match read_ctl(&mut s)? {
                    Ctl::Heartbeat => {
                        let _ = tx.send(Ctl::Heartbeat); // echo: the agent notices a dead relay
                    }
                    _ => return Err(NetError::Proto("unexpected frame from an agent")),
                }
            }
        })();
        alive.store(false, Ordering::SeqCst);
        if let Ok(mut agents) = self.agents.lock() {
            if agents.get(&id).map(|e| e.gen) == Some(gen) {
                agents.remove(&id);
            }
        }
        (self.log)(&format!("agent {id} gone"));
        let _ = s.shutdown(Shutdown::Both);
        result
    }

    fn viewer(self: &Arc<Self>, mut s: TcpStream, id: String, ip: IpAddr) -> Result<(), NetError> {
        let (tx, pending) = {
            let agents = self.agents.lock().map_err(|_| NetError::Proto("lock"))?;
            match agents.get(&id) {
                Some(e) => (e.tx.clone(), e.pending.clone()),
                None => {
                    drop(agents);
                    return write_ctl(&mut s, &Ctl::NotFound);
                }
            }
        };
        if pending.fetch_add(1, Ordering::SeqCst) >= self.limits.pending_per_agent {
            pending.fetch_sub(1, Ordering::SeqCst);
            return write_ctl(&mut s, &Ctl::Refused);
        }
        let outcome = (|| -> Result<Option<TcpStream>, NetError> {
            let token = random_token()?;
            let (stx, srx) = channel::<TcpStream>();
            self.waiting.lock().map_err(|_| NetError::Proto("lock"))?.insert(token, stx);
            if tx.send(Ctl::Incoming(token)).is_err() {
                self.waiting.lock().map_err(|_| NetError::Proto("lock"))?.remove(&token);
                return Ok(None);
            }
            let got = srx.recv_timeout(self.limits.pair_timeout).ok();
            if got.is_none() {
                self.waiting.lock().map_err(|_| NetError::Proto("lock"))?.remove(&token);
            }
            Ok(got)
        })();
        pending.fetch_sub(1, Ordering::SeqCst);
        let Some(mut a) = outcome? else {
            return write_ctl(&mut s, &Ctl::NotFound);
        };
        a.set_write_timeout(Some(Duration::from_secs(10)))?;
        write_ctl(&mut a, &Ctl::Paired)?;
        write_ctl(&mut s, &Ctl::Paired)?;
        (self.log)(&format!("bridge {ip} <-> agent {id}"));
        self.bridge(s, a)?;
        (self.log)(&format!("bridge {ip} <-> agent {id} ended"));
        Ok(())
    }

    /// Copy bytes both ways until one side ends or stays silent for `bridge_idle`.
    fn bridge(&self, a: TcpStream, b: TcpStream) -> Result<(), NetError> {
        for s in [&a, &b] {
            s.set_read_timeout(Some(self.limits.bridge_idle))?;
            s.set_write_timeout(Some(Duration::from_secs(30)))?;
        }
        let (mut a_r, mut b_w) = (a.try_clone()?, b.try_clone()?);
        let t = std::thread::Builder::new().name("relay-bridge".into()).spawn(move || {
            let _ = std::io::copy(&mut a_r, &mut b_w);
            let _ = a_r.shutdown(Shutdown::Both);
            let _ = b_w.shutdown(Shutdown::Both);
        })?;
        let (mut b_r, mut a_w) = (b.try_clone()?, a.try_clone()?);
        let _ = std::io::copy(&mut b_r, &mut a_w);
        let _ = a.shutdown(Shutdown::Both);
        let _ = b.shutdown(Shutdown::Both);
        let _ = t.join();
        Ok(())
    }
}
