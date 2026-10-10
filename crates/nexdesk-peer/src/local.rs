//! Is a target address this very computer? Controlling your own screen through a viewer makes the pointer
//! chase itself (the remote cursor *is* the local cursor), so the viewer starts with control paused then.
use std::net::{IpAddr, ToSocketAddrs, UdpSocket};

/// True when `addr` (`host`, `host:port`, `[v6]:port`) names a loopback address or one of this computer's own addresses.
pub fn is_this_computer(addr: &str) -> bool {
    let host = match addr.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None if addr.matches(':').count() > 1 => addr,
        None => addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr),
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let ips: Vec<IpAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => (host, 0u16)
            .to_socket_addrs()
            .map(|it| it.map(|s| s.ip()).collect())
            .unwrap_or_default(),
    };
    ips.iter().any(|ip| ip.is_loopback() || owns(*ip))
}

/// A UDP socket "connected" to our own address reports that same address as its local one (nothing is sent).
fn owns(ip: IpAddr) -> bool {
    if ip.is_unspecified() {
        return true;
    }
    let bind = if ip.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
    UdpSocket::bind(bind)
        .and_then(|s| s.connect((ip, 9)).map(|_| s))
        .and_then(|s| s.local_addr())
        .map(|a| a.ip() == ip)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_and_localhost_are_this_computer() {
        for a in [
            "127.0.0.1:21118",
            "127.0.0.1",
            "localhost:21117",
            "[::1]:21118",
            "::1",
            "0.0.0.0:1",
        ] {
            assert!(is_this_computer(a), "{a}");
        }
    }

    #[test]
    fn other_addresses_are_not() {
        for a in ["203.0.113.7:21118", "192.0.2.1", "example.invalid:5"] {
            assert!(!is_this_computer(a), "{a}");
        }
    }
}
