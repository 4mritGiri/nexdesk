//! Helpers for the NexDesk manager: run the viewer and the agent as child processes without a terminal.
//! Consent for an unknown agent is a two step flow: `probe` shows the fingerprint, the user decides,
//! then the viewer is started with `--trust <that fingerprint>`.
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};

/// Binary next to the running executable, else the one on `PATH`.
pub fn bin(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(name)))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// `host` or `host:port` made of plain characters only (never starts with `-`, no spaces).
pub fn valid_addr(a: &str) -> bool {
    !a.is_empty()
        && a.len() <= 255
        && !a.starts_with('-')
        && a.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_' | '[' | ']'))
}

/// True for `0.0.0.0` / `::` (with or without a port): addresses a server *listens* on, which mean
/// "this computer" when used as a destination. Typing one as the relay never reaches a relay on another computer.
pub fn is_wildcard_host(addr: &str) -> bool {
    let host = match addr.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None if addr.matches(':').count() > 1 => addr, // bare IPv6
        None => addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr),
    };
    matches!(host, "0.0.0.0" | "::" | "0:0:0:0:0:0:0:0")
}

/// `SHA256:` followed by base64 characters.
pub fn valid_fingerprint(f: &str) -> bool {
    f.strip_prefix("SHA256:")
        // Real fingerprints are `SHA256:` + 8 groups of 8 hex digits joined by '-' (71 characters).
        .map(|r| {
            !r.is_empty()
                && r.len() <= 96
                && r.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-'))
        })
        .unwrap_or(false)
}

/// Add the default port when `addr` has none.
pub fn with_port(addr: &str) -> String {
    match addr.rsplit_once(':') {
        Some((_, p)) if p.parse::<u16>().is_ok() => addr.to_string(),
        _ => format!("{}:{}", addr, crate::DEFAULT_PORT),
    }
}

/// Connect once to learn the agent's fingerprint. Blocking: call it from a thread.
pub fn probe(addr: &str, relay: Option<&str>) -> Result<String, String> {
    if !valid_addr(addr) || relay.map(|r| !valid_addr(r)).unwrap_or(false) {
        return Err("that is not a valid address".into());
    }
    let mut cmd = Command::new(bin("nexdesk-peer-view"));
    if let Some(r) = relay {
        cmd.args(["--relay", r]);
    }
    let out = cmd
        .args(["--probe", addr])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot start nexdesk-peer-view: {e}"))?;
    let so = String::from_utf8_lossy(&out.stdout);
    if let Some(fp) = so.lines().find_map(|l| l.strip_prefix("FINGERPRINT ")) {
        let fp = fp.trim();
        if valid_fingerprint(fp) {
            return Ok(fp.to_string());
        }
    }
    let err = String::from_utf8_lossy(&out.stderr);
    match err.lines().rev().find(|l| !l.trim().is_empty()) {
        Some(l) => Err(l.trim().to_string()),
        // No answer and no explanation: almost always an old nexdesk-peer-view next to a newer manager.
        None => Err(format!(
            "nexdesk-peer-view stopped without saying why ({}). It is probably an old build: run 'cargo build --workspace' (or reinstall) so every NexDesk program is up to date, then try again.",
            out.status
        )),
    }
}

/// Open a viewer window. `trust` pins the agent if (and only if) its fingerprint is exactly this.
pub fn launch_viewer(
    addr: &str,
    relay: Option<&str>,
    trust: Option<&str>,
    clipboard: bool,
) -> Result<Child, String> {
    if !valid_addr(addr)
        || trust.map(|t| !valid_fingerprint(t)).unwrap_or(false)
        || relay.map(|r| !valid_addr(r)).unwrap_or(false)
    {
        return Err("invalid address or fingerprint".into());
    }
    let mut c = Command::new(bin("nexdesk-peer-view"));
    if let Some(r) = relay {
        c.args(["--relay", r]);
    }
    c.arg(addr);
    if let Some(t) = trust {
        c.args(["--trust", t]);
    }
    if !clipboard {
        c.arg("--no-clipboard");
    }
    c.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    c.spawn()
        .map_err(|e| format!("cannot start nexdesk-peer-view: {e}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    Identity(String),
    /// This computer's relay ID (nine digits).
    Id(String),
    /// Relay registration status text, for display only.
    Relay(String),
    /// A viewer asks to connect; answer with `Agent::answer`.
    Request(String),
    /// The connected viewer wants to send files: (file count, bytes, first name). Answer with `answer_files`.
    Files(u32, u64, String),
    /// A chat line from the viewer.
    Chat(String),
    /// A warning the person should read (for example: this desktop is Wayland).
    Warn(String),
    Exited,
}

/// The agent running as a child of the manager, asking for consent over stdin/stdout.
pub struct Agent {
    child: Child,
    stdin: ChildStdin,
    pub events: Receiver<AgentEvent>,
}

impl Agent {
    pub fn start(
        listen: &str,
        relay: Option<&str>,
        view_only: bool,
        clipboard: bool,
    ) -> Result<Self, String> {
        if !valid_addr(listen) || relay.map(|r| !valid_addr(r)).unwrap_or(false) {
            return Err("invalid listen or relay address".into());
        }
        let mut c = Command::new(bin("nexdesk-agent"));
        c.args(["--listen", listen, "--stdio-control"]);
        if let Some(r) = relay {
            c.args(["--relay", r]);
        }
        if view_only {
            c.arg("--view-only");
        }
        if !clipboard {
            c.arg("--no-clipboard");
        }
        let mut child = c
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start nexdesk-agent: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let ev = match line.split_once(' ') {
                    Some(("IDENTITY", f)) if valid_fingerprint(f.trim()) => {
                        AgentEvent::Identity(f.trim().into())
                    }
                    Some(("ID", i)) if nexdesk_network::proto::valid_id(i.trim()) => {
                        AgentEvent::Id(i.trim().into())
                    }
                    Some(("RELAY", t)) => {
                        AgentEvent::Relay(t.chars().filter(|c| !c.is_control()).take(120).collect())
                    }
                    Some(("REQUEST", f)) if valid_fingerprint(f.trim()) => {
                        AgentEvent::Request(f.trim().into())
                    }
                    Some(("WARN", t)) => {
                        AgentEvent::Warn(t.chars().filter(|c| !c.is_control()).take(300).collect())
                    }
                    Some(("CHAT", t)) => AgentEvent::Chat(
                        t.chars()
                            .filter(|c| !c.is_control())
                            .take(crate::wire::MAX_CHAT)
                            .collect(),
                    ),
                    Some(("FILES", t)) => {
                        let mut p = t.splitn(3, ' ');
                        match (
                            p.next().and_then(|c| c.parse().ok()),
                            p.next().and_then(|b| b.parse().ok()),
                        ) {
                            (Some(c), Some(b)) => AgentEvent::Files(
                                c,
                                b,
                                p.next()
                                    .unwrap_or("")
                                    .chars()
                                    .filter(|c| !c.is_control())
                                    .take(120)
                                    .collect(),
                            ),
                            _ => continue,
                        }
                    }
                    _ => continue,
                };
                if tx.send(ev).is_err() {
                    return;
                }
            }
            let _ = tx.send(AgentEvent::Exited);
        });
        Ok(Self {
            child,
            stdin,
            events: rx,
        })
    }

    pub fn answer(&mut self, yes: bool) {
        let _ = writeln!(self.stdin, "{}", if yes { "yes" } else { "no" });
        let _ = self.stdin.flush();
    }

    pub fn answer_files(&mut self, yes: bool) {
        let _ = writeln!(self.stdin, "files {}", if yes { "yes" } else { "no" });
        let _ = self.stdin.flush();
    }

    /// Send a chat line to the connected viewer.
    pub fn send_chat(&mut self, text: &str) {
        let t = crate::wire::clean_chat(text);
        if !t.is_empty() {
            let _ = writeln!(self.stdin, "chat {t}");
            let _ = self.stdin.flush();
        }
    }

    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_and_fingerprint_validation() {
        assert!(
            valid_addr("192.168.1.5:21118") && valid_addr("pc-1.lan") && valid_addr("[::1]:21118")
        );
        for bad in ["", "-x", "--trust", "a b", "a;rm", "a\n", "$(x)"] {
            assert!(!valid_addr(bad), "{bad:?}");
        }
        assert!(valid_fingerprint("SHA256:abcDEF123+/="));
        // the real format produced by nexdesk-crypto
        assert!(valid_fingerprint(
            "SHA256:8605fbfb-d517b58d-caadb6c0-38aeebe8-a3414b6a-656125bc-1a879f7d-86e47e1f"
        ));
        let real = nexdesk_crypto::Identity::generate()
            .unwrap()
            .public()
            .fingerprint_string();
        assert!(
            valid_fingerprint(&real),
            "a real fingerprint must be accepted: {real}"
        );
        for bad in ["", "SHA256:", "MD5:abc", "SHA256:a b", "SHA256:--x;"] {
            assert!(!valid_fingerprint(bad), "{bad:?}");
        }
        assert!(
            is_wildcard_host("0.0.0.0")
                && is_wildcard_host("0.0.0.0:21117")
                && is_wildcard_host("[::]:21117")
                && is_wildcard_host("::")
        );
        assert!(
            !is_wildcard_host("192.168.1.20:21117")
                && !is_wildcard_host("relay.example.com")
                && !is_wildcard_host("[::1]:21117")
        );
        assert_eq!(with_port("pc"), "pc:21118");
        assert_eq!(with_port("pc:99"), "pc:99");
    }
}
