//! One viewer window for all sessions. The first `nexdesk-rdp` process owns the window and
//! listens on a private local socket; later ones hand their connection (including the password,
//! over that socket only) to it and wait, so each session shows up as a tab.
//!
//! The socket lives in `$XDG_RUNTIME_DIR` (mode 0700) and is chmod 0600; the password is never
//! put on a command line or in a log.
use std::io::{BufRead, BufReader, Read, Write};

use nexdesk_core::profiles::Speed;

use crate::tls::Policy;

/// Everything needed to open one session.
#[derive(Clone)]
pub struct Spec {
    pub host: String,
    pub user: String,
    pub domain: Option<String>,
    pub password: String,
    pub width: u16,
    pub height: u16,
    pub dynamic_resize: bool,
    pub fullscreen: bool,
    pub capture_keys: bool,
    pub clipboard: bool,
    pub drop_paste: bool,
    pub tls: Policy,
    pub speed: Speed,
    pub forget_host: bool,
    /// Name shown on the tab and in the activity log.
    pub profile: String,
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n")
}

fn unesc(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl Spec {
    pub fn encode(&self) -> String {
        let b = |v: bool| if v { "1" } else { "0" };
        let mut s = String::new();
        let mut put = |k: &str, v: &str| {
            s.push_str(k);
            s.push('=');
            s.push_str(&esc(v));
            s.push('\n');
        };
        put("host", &self.host);
        put("user", &self.user);
        put("domain", self.domain.as_deref().unwrap_or(""));
        put("password", &self.password);
        put("width", &self.width.to_string());
        put("height", &self.height.to_string());
        put("dynamic_resize", b(self.dynamic_resize));
        put("fullscreen", b(self.fullscreen));
        put("capture_keys", b(self.capture_keys));
        put("clipboard", b(self.clipboard));
        put("drop_paste", b(self.drop_paste));
        put("tls", self.tls.as_str());
        put("speed", self.speed.cli());
        put("forget_host", b(self.forget_host));
        put("profile", &self.profile);
        s.push_str(".\n");
        s
    }

    /// Read `key=value` lines up to the "." terminator.
    pub fn read(r: &mut impl BufRead) -> Result<Self, String> {
        let mut m: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for _ in 0..64 {
            let mut line = String::new();
            if r.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                return Err("connection closed".into());
            }
            let line = line.trim_end_matches('\n');
            if line == "." {
                let get = |k: &str| m.get(k).cloned().unwrap_or_default();
                let flag = |k: &str| get(k) == "1";
                let host = get("host");
                if host.is_empty() || host.len() > 300 {
                    return Err("bad host".into());
                }
                let dim = |k: &str| {
                    get(k)
                        .parse::<u16>()
                        .ok()
                        .filter(|v| (200..=8192).contains(v))
                        .ok_or(format!("bad {k}"))
                };
                return Ok(Spec {
                    host,
                    user: get("user"),
                    domain: Some(get("domain")).filter(|d| !d.is_empty()),
                    password: get("password"),
                    width: dim("width")?,
                    height: dim("height")?,
                    dynamic_resize: flag("dynamic_resize"),
                    fullscreen: flag("fullscreen"),
                    capture_keys: flag("capture_keys"),
                    clipboard: flag("clipboard"),
                    drop_paste: flag("drop_paste"),
                    tls: Policy::parse(&get("tls")).unwrap_or(Policy::Ask),
                    speed: Speed::parse(&get("speed")).unwrap_or_default(),
                    forget_host: flag("forget_host"),
                    profile: get("profile"),
                });
            }
            if let Some((k, v)) = line.split_once('=') {
                m.insert(k.to_owned(), unesc(v));
            }
        }
        Err("request too long".into())
    }
}

#[cfg(unix)]
mod sock {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;

    pub fn path() -> PathBuf {
        if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR") {
            return PathBuf::from(d).join("nexdesk-rdp.sock");
        }
        let uid = std::fs::metadata("/proc/self")
            .map(|m| m.uid())
            .unwrap_or(0);
        PathBuf::from(format!("/tmp/nexdesk-rdp-{uid}.sock"))
    }

    pub fn connect() -> Option<UnixStream> {
        UnixStream::connect(path()).ok()
    }

    pub fn bind() -> Option<UnixListener> {
        let p = path();
        for _ in 0..2 {
            match UnixListener::bind(&p) {
                Ok(l) => {
                    let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
                    return Some(l);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                    if UnixStream::connect(&p).is_ok() {
                        return None; // somebody else is the window owner
                    }
                    let _ = std::fs::remove_file(&p); // stale socket from a crashed run
                }
                Err(_) => return None,
            }
        }
        None
    }

    pub fn cleanup() {
        let _ = std::fs::remove_file(path());
    }
}

/// Ask the running NexDesk manager window to come to the front. `false` when no manager is listening.
/// Same socket location rule as `nexdesk-ui/src/instance.rs`; keep the two in step.
#[cfg(unix)]
pub fn focus_manager() -> bool {
    use std::os::unix::fs::MetadataExt;
    let path = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) => std::path::PathBuf::from(d).join("nexdesk-manager.sock"),
        None => {
            let uid = std::env::var_os("HOME")
                .and_then(|h| std::fs::metadata(h).ok())
                .map(|m| m.uid())
                .unwrap_or(0);
            std::path::PathBuf::from(format!("/tmp/nexdesk-manager-{uid}.sock"))
        }
    };
    std::os::unix::net::UnixStream::connect(path)
        .and_then(|mut s| s.write_all(b"focus\n"))
        .is_ok()
}
#[cfg(not(unix))]
pub fn focus_manager() -> bool {
    false
}

#[cfg(unix)]
pub use sock::cleanup;
#[cfg(not(unix))]
pub fn cleanup() {}

/// Hand the session to a running window. `Some(exit code)` when it took over (we then wait
/// until the tab closes); `None` when there is no window yet and the caller should open one.
#[cfg(unix)]
pub fn forward(spec: &Spec) -> Option<i32> {
    let mut s = sock::connect()?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .ok()?;
    s.write_all(spec.encode().as_bytes()).ok()?;
    let mut r = BufReader::new(s.try_clone().ok()?);
    let mut line = String::new();
    if r.read_line(&mut line).ok()? == 0 {
        return None; // the window went away while we connected: open our own
    }
    if let Some(msg) = line.trim_end().strip_prefix("ERR ") {
        eprintln!("Error: {msg}");
        return Some(1);
    }
    s.set_read_timeout(None).ok()?;
    line.clear();
    match r.read_line(&mut line) {
        Ok(n) if n > 0 && line.starts_with("END") => Some(0),
        Ok(n) if n > 0 => {
            eprintln!("Error: {}", line.trim_end().trim_start_matches("FAIL "));
            Some(1)
        }
        _ => Some(1),
    }
}

#[cfg(not(unix))]
pub fn forward(_spec: &Spec) -> Option<i32> {
    None
}

#[cfg(unix)]
pub fn bind() -> Option<std::os::unix::net::UnixListener> {
    sock::bind()
}
#[cfg(not(unix))]
pub fn bind() -> Option<()> {
    None
}

/// Accept hand-offs from later processes and turn them into `UserEvent`s.
#[cfg(unix)]
pub fn serve(
    listener: std::os::unix::net::UnixListener,
    proxy: winit::event_loop::EventLoopProxy<crate::app::UserEvent>,
) {
    use crate::app::UserEvent;
    use std::sync::mpsc;
    let _ = std::thread::Builder::new()
        .name("ipc-accept".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let proxy = proxy.clone();
                let _ = std::thread::Builder::new()
                    .name("ipc-client".into())
                    .spawn(move || {
                        let Ok(rd) = stream.try_clone() else { return };
                        let mut w = stream;
                        let mut reader = BufReader::new(rd.take(64 * 1024));
                        let spec = match Spec::read(&mut reader) {
                            Ok(s) => s,
                            Err(e) => {
                                let _ = writeln!(w, "ERR {e}");
                                return;
                            }
                        };
                        let (ack_tx, ack_rx) = mpsc::channel();
                        let (end_tx, end_rx) = mpsc::channel();
                        if proxy
                            .send_event(UserEvent::NewSession {
                                spec,
                                ack: ack_tx,
                                end: end_tx,
                            })
                            .is_err()
                        {
                            return;
                        }
                        let id = match ack_rx.recv_timeout(std::time::Duration::from_secs(30)) {
                            Ok(Ok(id)) => id,
                            Ok(Err(e)) => {
                                let _ = writeln!(w, "ERR {}", e.replace('\n', " "));
                                return;
                            }
                            Err(_) => {
                                let _ = writeln!(w, "ERR the viewer window did not answer");
                                return;
                            }
                        };
                        let _ = writeln!(w, "OK");
                        // If the other process is killed (the manager's Disconnect), close the tab.
                        let watcher_proxy = proxy.clone();
                        let mut watch = reader.into_inner().into_inner();
                        let _ =
                            std::thread::Builder::new()
                                .name("ipc-watch".into())
                                .spawn(move || {
                                    let mut buf = [0u8; 64];
                                    while matches!(watch.read(&mut buf), Ok(n) if n > 0) {}
                                    let _ = watcher_proxy.send_event(UserEvent::CloseSession(id));
                                });
                        match end_rx.recv() {
                            Ok(Ok(())) => {
                                let _ = writeln!(w, "END");
                            }
                            Ok(Err(m)) => {
                                let _ = writeln!(w, "FAIL {}", m.replace('\n', " "));
                            }
                            Err(_) => {}
                        }
                    });
            }
        });
}

#[cfg(not(unix))]
pub fn serve(_l: (), _p: winit::event_loop::EventLoopProxy<crate::app::UserEvent>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_round_trip_keeps_odd_characters() {
        let s = Spec {
            host: "10.0.0.5:3390".into(),
            user: "ann".into(),
            domain: None,
            password: "p\\a\nss=w0rd".into(),
            width: 1920,
            height: 1080,
            dynamic_resize: true,
            fullscreen: false,
            capture_keys: true,
            clipboard: true,
            drop_paste: false,
            tls: Policy::Strict,
            speed: Speed::Slow,
            forget_host: false,
            profile: "My PC".into(),
        };
        let text = s.encode();
        let back = Spec::read(&mut text.as_bytes()).unwrap();
        assert_eq!(back.password, s.password);
        assert_eq!(back.host, s.host);
        assert_eq!(back.tls, Policy::Strict);
        assert_eq!(back.speed, Speed::Slow);
        assert!(back.domain.is_none());
        assert!(back.dynamic_resize && !back.fullscreen);
    }

    #[test]
    fn spec_rejects_garbage() {
        assert!(Spec::read(&mut "host=\n.\n".as_bytes()).is_err());
        assert!(Spec::read(&mut "host=a\nwidth=1\nheight=1\n.\n".as_bytes()).is_err());
        assert!(Spec::read(&mut "".as_bytes()).is_err());
    }
}
