//! A tiny local socket so other NexDesk programs can talk to the running manager window.
//! Two one-line messages exist:
//!
//! * `focus`: bring the window to the front (the session window's "+" button);
//! * `connect <name>`: select the saved connection `<name>` and show its password prompt
//!   (`nexdesk --connect <name>` from a launcher such as Aujar). No password, address or other
//!   connection detail is ever sent, only the name of a connection the user already saved.
//!
//! Same socket location rule as `nexdesk-rdp/src/ipc.rs`; keep the two in step.
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Set when something wants this window in front: a "focus" message, or a question that needs an answer.
static RAISE: AtomicBool = AtomicBool::new(false);

/// A connection name waiting to be opened by the window.
static CONNECT: Mutex<Option<String>> = Mutex::new(None);

/// Longest connection name the manager accepts (`profiles::file_stem_for` allows 100 characters).
const MAX_NAME_CHARS: usize = 100;

/// Longest message line read from the socket: `connect ` plus a 100-character name in UTF-8.
const MAX_LINE_BYTES: u64 = 512;

pub fn raise() {
    RAISE.store(true, Ordering::SeqCst);
}

/// A name from the socket is data from another process: keep it short and free of control
/// characters before it reaches the UI.
fn clean_name(name: &str) -> Option<String> {
    let name = name.trim();
    let too_long = name.chars().count() > MAX_NAME_CHARS;
    if name.is_empty() || too_long || name.chars().any(char::is_control) {
        None
    } else {
        Some(name.to_string())
    }
}

/// Ask the window to open the saved connection `name`, and to come to the front.
pub fn request_connect(name: &str) -> bool {
    let Some(name) = clean_name(name) else {
        return false;
    };
    *CONNECT.lock().unwrap_or_else(|p| p.into_inner()) = Some(name);
    raise();
    true
}

/// The pending connection name, once per request; the window polls this next to `take_raise`.
pub fn take_connect() -> Option<String> {
    CONNECT.lock().unwrap_or_else(|p| p.into_inner()).take()
}

/// Send one message line to the manager that is already running. `false` when none answers.
pub fn send(message: &str) -> bool {
    let Ok(mut stream) = UnixStream::connect(path()) else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(2)));
    stream.write_all(format!("{message}\n").as_bytes()).is_ok()
}

/// What one received line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Message {
    Focus,
    Connect(String),
}

fn parse_message(line: &str) -> Option<Message> {
    let line = line.trim();
    if line == "focus" {
        return Some(Message::Focus);
    }
    clean_name(line.strip_prefix("connect ")?).map(Message::Connect)
}

/// True once per request; the window polls this.
pub fn take_raise() -> bool {
    RAISE.swap(false, Ordering::SeqCst)
}

fn path() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(d).join("nexdesk-manager.sock");
    }
    let uid = std::env::var_os("HOME")
        .and_then(|h| std::fs::metadata(h).ok())
        .map(|m| m.uid())
        .unwrap_or(0);
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
                // a short line: anything longer is not ours
                let read = BufReader::new(conn.take(MAX_LINE_BYTES)).read_line(&mut line);
                if read.is_err() {
                    continue;
                }
                match parse_message(&line) {
                    Some(Message::Focus) => raise(),
                    Some(Message::Connect(name)) => {
                        request_connect(&name);
                    }
                    None => {}
                }
            }
        })
        .is_ok()
}

pub fn cleanup() {
    let _ = std::fs::remove_file(path());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_and_connect_lines_are_understood() {
        assert_eq!(parse_message("focus\n"), Some(Message::Focus));
        assert_eq!(
            parse_message("connect Office PC\n"),
            Some(Message::Connect("Office PC".into()))
        );
        assert_eq!(
            parse_message("connect   padded  \n"),
            Some(Message::Connect("padded".into()))
        );
    }

    #[test]
    fn anything_else_is_ignored() {
        assert_eq!(parse_message(""), None);
        assert_eq!(parse_message("connect"), None);
        assert_eq!(parse_message("connect "), None);
        assert_eq!(parse_message("connect \u{7}bell"), None);
        assert_eq!(parse_message("connect a\tb"), None);
        assert_eq!(parse_message("quit"), None);
        assert_eq!(parse_message(&format!("connect {}", "x".repeat(101))), None);
    }

    #[test]
    fn a_hundred_character_name_is_the_longest_accepted() {
        assert!(clean_name(&"x".repeat(100)).is_some());
        assert!(clean_name(&"x".repeat(101)).is_none());
    }

    #[test]
    fn a_connect_request_is_taken_exactly_once() {
        assert!(request_connect("Server 1"));
        assert_eq!(take_connect().as_deref(), Some("Server 1"));
        assert_eq!(take_connect(), None);
        assert!(!request_connect("   "));
        assert_eq!(take_connect(), None);
    }
}
