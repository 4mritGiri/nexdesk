//! Turn low-level connection errors into something a person can act on.
use std::error::Error;

/// The error plus every underlying cause, joined with ": ".
pub fn chain(e: &dyn Error) -> String {
    let mut parts = vec![e.to_string()];
    let mut cur = e.source();
    while let Some(s) = cur {
        let t = s.to_string();
        if !parts.iter().any(|p| p == &t) {
            parts.push(t);
        }
        cur = s.source();
    }
    parts.join(": ")
}

/// A plain-language hint for the most common failures, if we recognise one.
pub fn hint(msg: &str) -> Option<&'static str> {
    let m = msg.to_ascii_lowercase();
    if m.contains("refused") {
        Some("The computer answered but nothing listens on the RDP port. On that Windows PC turn on Settings > System > Remote Desktop (Pro/Enterprise/Server editions only; Windows Home cannot host RDP) and check the port (default 3389).")
    } else if m.contains("timed out") || m.contains("timeout") {
        Some("No answer. The computer may be asleep or off, on another network, or a firewall drops port 3389. On Windows allow 'Remote Desktop' in Windows Defender Firewall for the Private profile.")
    } else if m.contains("no route") || m.contains("unreachable") || m.contains("name or service not known") || m.contains("failed to lookup") || m.contains("resolve") {
        Some("The address cannot be reached. Check the host name or IP address and that you are on the same network (or the VPN is up).")
    } else if m.contains("credssp") || m.contains("logon") || m.contains("credential") || m.contains("access denied") || m.contains("0xc000006d") {
        Some("The computer rejected the sign-in. Check user name and password; for a Microsoft account use the email address, for a local account use .\\name or the PC name as domain. The account must be allowed under 'Select users that can remotely access this PC'.")
    } else if m.contains("another user connected") {
        Some("Someone signed in on that computer and took over the session. Windows allows only one remote user at a time on non-server editions.")
    } else {
        None
    }
}

/// Message shown to the user: cause chain plus hint.
pub fn explain(prefix: &str, e: &dyn Error) -> String {
    let c = chain(e);
    match hint(&c) {
        Some(h) => format!("{prefix}: {c}\n\n{h}"),
        None => format!("{prefix}: {c}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;

    #[derive(Debug)]
    struct Outer(std::io::Error);
    impl fmt::Display for Outer {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "[TCP connect] custom error")
        }
    }
    impl Error for Outer {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn chain_includes_the_root_cause() {
        let e = Outer(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "Connection refused (os error 111)"));
        let s = explain("connection failed", &e);
        assert!(s.contains("custom error: Connection refused"), "{s}");
        assert!(s.contains("Remote Desktop"), "{s}");
    }

    #[test]
    fn hints() {
        assert!(hint("Connection timed out (os error 110)").unwrap().contains("firewall"));
        assert!(hint("No route to host").unwrap().contains("same network"));
        assert!(hint("CredSSP failure").unwrap().contains("user name"));
        assert!(hint("something odd").is_none());
    }
}
