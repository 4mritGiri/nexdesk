//! A tiny local socket so other NexDesk programs (the session window's "+" button) can ask the
//! running manager window to come to the front. The only message is the word `focus`.
//! Same socket location rule as `nexdesk-rdp/src/ipc.rs`; keep the two in step.
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set when something wants this window in front: a "focus" message, or a question that needs an answer.
static RAISE: AtomicBool = AtomicBool::new(false);

pub fn raise() {
    RAISE.store(true, Ordering::SeqCst);
}

/// True once per request; the window polls this.
pub fn take_raise() -> bool {
    RAISE.swap(false, Ordering::SeqCst)
}

fn path() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(d).join("nexdesk-manager.sock");
    }
    let uid = std::env::var_os("HOME").and_then(|h| std::fs::metadata(h).ok()).map(|m| m.uid()).unwrap_or(0);
    PathBuf::from(format!("/tmp/nexdesk-manager-{uid}.sock"))
}

/// Start listening; each "focus" message calls `raise()`.
/// Returns false when another manager already owns the socket (that one keeps answering).
pub fn listen() -> bool {
    let p = path();
    let listener = match UnixListener::bind(&p) {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if std::os::unix::net::UnixStream::connect(&p).is_ok() {
                return false;
            }
            let _ = std::fs::remove_file(&p); // stale socket from a crashed run
            match UnixListener::bind(&p) {
                Ok(l) => l,
                Err(_) => return false,
            }
        }
        Err(_) => return false,
    };
    let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
    std::thread::Builder::new()
        .name("manager-focus".into())
        .spawn(move || {
            for conn in listener.incoming().flatten() {
                let _ = conn.set_read_timeout(Some(std::time::Duration::from_secs(2)));
                let mut line = String::new();
                // at most 64 bytes: anything longer is not ours
                let read = BufReader::new(conn.take(64)).read_line(&mut line);
                if read.is_ok() && line.trim() == "focus" {
                    raise();
                }
            }
        })
        .is_ok()
}

pub fn cleanup() {
    let _ = std::fs::remove_file(path());
}
