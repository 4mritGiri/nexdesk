//! Find computers on the local network that accept RDP connections.
//!
//! Only the machine's own IPv4 subnets are probed (never more than one /24 per interface),
//! only the RDP port is tried, and only when the user asks. A host counts as "found" when a
//! TCP connection to the port succeeds; nothing is sent after connecting.
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const RDP_PORT: u16 = 3389;
const MAX_HOSTS: usize = 254;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub ip: Ipv4Addr,
    /// Reverse-DNS name, when the network provides one.
    pub name: Option<String>,
    /// This computer itself.
    pub is_self: bool,
}

/// Addresses to probe for `ip/prefix`: the whole subnet, but at most the /24 around `ip`.
pub fn hosts_in(ip: Ipv4Addr, prefix: u8) -> Vec<Ipv4Addr> {
    if prefix >= 31 || prefix == 0 {
        return Vec::new();
    }
    let prefix = prefix.max(24);
    let mask = u32::MAX << (32 - prefix);
    let net = u32::from(ip) & mask;
    let bcast = net | !mask;
    ((net + 1)..bcast).map(Ipv4Addr::from).take(MAX_HOSTS).collect()
}

fn usable(ip: Ipv4Addr) -> bool {
    !(ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_multicast())
}

/// (address, prefix length) of every up, non-loopback IPv4 interface.
#[cfg(unix)]
pub fn local_subnets() -> Vec<(Ipv4Addr, u8)> {
    let mut out = Vec::new();
    let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut ifap) } != 0 {
        return out;
    }
    let mut cur = ifap;
    while !cur.is_null() {
        let ifa = unsafe { &*cur };
        cur = ifa.ifa_next;
        let up = ifa.ifa_flags & (libc::IFF_UP as u32) != 0 && ifa.ifa_flags & (libc::IFF_LOOPBACK as u32) == 0;
        if !up || ifa.ifa_addr.is_null() || ifa.ifa_netmask.is_null() {
            continue;
        }
        if unsafe { (*ifa.ifa_addr).sa_family } as i32 != libc::AF_INET {
            continue;
        }
        let (a, m) = unsafe {
            (
                &*(ifa.ifa_addr as *const libc::sockaddr_in),
                &*(ifa.ifa_netmask as *const libc::sockaddr_in),
            )
        };
        let ip = Ipv4Addr::from(u32::from_be(a.sin_addr.s_addr));
        let prefix = u32::from_be(m.sin_addr.s_addr).count_ones() as u8;
        if usable(ip) && !out.contains(&(ip, prefix)) {
            out.push((ip, prefix));
        }
    }
    unsafe { libc::freeifaddrs(ifap) };
    out
}

#[cfg(not(unix))]
pub fn local_subnets() -> Vec<(Ipv4Addr, u8)> {
    Vec::new()
}

#[cfg(unix)]
fn reverse_dns(ip: Ipv4Addr) -> Option<String> {
    let sa = libc::sockaddr_in {
        sin_family: libc::AF_INET as libc::sa_family_t,
        sin_port: 0,
        sin_addr: libc::in_addr { s_addr: u32::from(ip).to_be() },
        sin_zero: [0; 8],
    };
    let mut buf = [0 as libc::c_char; 256];
    let rc = unsafe {
        libc::getnameinfo(
            &sa as *const _ as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            buf.as_mut_ptr(),
            buf.len() as libc::socklen_t,
            std::ptr::null_mut(),
            0,
            libc::NI_NAMEREQD,
        )
    };
    if rc != 0 {
        return None;
    }
    let s = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned();
    if s.is_empty() { None } else { Some(s) }
}

#[cfg(not(unix))]
fn reverse_dns(_: Ipv4Addr) -> Option<String> {
    None
}

/// A running (or finished) scan. Poll `results()` / `progress()` from the UI.
#[derive(Clone)]
pub struct Scan {
    found: Arc<Mutex<Vec<Found>>>,
    done: Arc<AtomicBool>,
    checked: Arc<AtomicUsize>,
    pub total: usize,
}

impl Scan {
    /// Scan the local subnets.
    pub fn start() -> Self {
        let mut targets: Vec<(Ipv4Addr, bool)> = Vec::new();
        let own: Vec<Ipv4Addr> = local_subnets().iter().map(|s| s.0).collect();
        for (ip, prefix) in local_subnets() {
            for h in hosts_in(ip, prefix) {
                if !targets.iter().any(|t| t.0 == h) {
                    targets.push((h, own.contains(&h)));
                }
            }
        }
        Self::start_with(targets, RDP_PORT, Duration::from_millis(600))
    }

    /// Scan explicit targets (also used by tests).
    pub fn start_with(targets: Vec<(Ipv4Addr, bool)>, port: u16, timeout: Duration) -> Self {
        let scan = Scan {
            found: Arc::new(Mutex::new(Vec::new())),
            done: Arc::new(AtomicBool::new(false)),
            checked: Arc::new(AtomicUsize::new(0)),
            total: targets.len(),
        };
        if targets.is_empty() {
            scan.done.store(true, Ordering::SeqCst);
            return scan;
        }
        let queue = Arc::new(Mutex::new(targets));
        let workers = 48usize.min(scan.total);
        let live = Arc::new(AtomicUsize::new(workers));
        for _ in 0..workers {
            let (queue, found, checked, done, live) =
                (queue.clone(), scan.found.clone(), scan.checked.clone(), scan.done.clone(), live.clone());
            std::thread::spawn(move || {
                loop {
                    let next = queue.lock().ok().and_then(|mut q| q.pop());
                    let Some((ip, is_self)) = next else { break };
                    let addr = SocketAddr::new(IpAddr::V4(ip), port);
                    if TcpStream::connect_timeout(&addr, timeout).is_ok() {
                        let f = Found { ip, name: reverse_dns(ip), is_self };
                        if let Ok(mut g) = found.lock() {
                            g.push(f);
                            g.sort_by_key(|f| u32::from(f.ip));
                        }
                    }
                    checked.fetch_add(1, Ordering::SeqCst);
                }
                if live.fetch_sub(1, Ordering::SeqCst) == 1 {
                    done.store(true, Ordering::SeqCst);
                }
            });
        }
        scan
    }

    pub fn results(&self) -> Vec<Found> {
        self.found.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    /// (checked, total)
    pub fn progress(&self) -> (usize, usize) {
        (self.checked.load(Ordering::SeqCst).min(self.total), self.total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subnet_expansion() {
        let h = hosts_in(Ipv4Addr::new(192, 168, 1, 77), 24);
        assert_eq!(h.len(), 254);
        assert_eq!(h[0], Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(h[253], Ipv4Addr::new(192, 168, 1, 254));
        // a /16 is limited to the /24 around us
        let h = hosts_in(Ipv4Addr::new(10, 0, 5, 9), 16);
        assert_eq!(h.len(), 254);
        assert!(h.iter().all(|a| a.octets()[2] == 5));
        // small subnet
        let h = hosts_in(Ipv4Addr::new(192, 168, 1, 130), 28);
        assert_eq!(h.first(), Some(&Ipv4Addr::new(192, 168, 1, 129)));
        assert_eq!(h.last(), Some(&Ipv4Addr::new(192, 168, 1, 142)));
        assert!(hosts_in(Ipv4Addr::new(1, 2, 3, 4), 32).is_empty());
        assert!(hosts_in(Ipv4Addr::new(1, 2, 3, 4), 0).is_empty());
    }

    #[test]
    fn finds_only_hosts_with_the_port_open() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let scan = Scan::start_with(vec![(Ipv4Addr::LOCALHOST, true)], port, Duration::from_millis(500));
        for _ in 0..100 {
            if scan.is_done() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(scan.is_done());
        let r = scan.results();
        assert_eq!(r.len(), 1);
        assert!(r[0].is_self);
        assert_eq!(scan.progress(), (1, 1));
        drop(l);
        let closed = Scan::start_with(vec![(Ipv4Addr::LOCALHOST, false)], port, Duration::from_millis(300));
        while !closed.is_done() {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(closed.results().is_empty());
        assert!(Scan::start_with(vec![], 1, Duration::from_millis(1)).is_done());
    }
}
