//! Activity logs shown in the manager (Connection, File, Alarm, Console) and written by the
//! manager, the session engine and the clipboard engine.
//!
//! Plain tab-separated text, one file per kind in `~/.local/share/nexdesk/logs/`. Appends are a
//! single `write` of one short line, so several processes can write at once. Files rotate at
//! 2 MiB (one `.1` backup). **Never log passwords, clipboard contents or file contents** - only
//! names, sizes, hosts and outcomes.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Connection,
    File,
    Alarm,
    Console,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Connection, Kind::File, Kind::Alarm, Kind::Console];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Connection => "Connection",
            Kind::File => "File",
            Kind::Alarm => "Alarm",
            Kind::Console => "Console",
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Kind::Connection => "connection.log",
            Kind::File => "file.log",
            Kind::Alarm => "alarm.log",
            Kind::Console => "console.log",
        }
    }

    /// Column titles for the fields of this kind (the time and level columns come first).
    pub fn columns(self) -> &'static [&'static str] {
        match self {
            Kind::Connection => &["Event", "Connection", "Computer", "User", "Detail"],
            Kind::File => &["Direction", "Items", "Size", "Status", "Computer"],
            Kind::Alarm => &["Alarm", "Computer", "Detail"],
            Kind::Console => &["Source", "Message"],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn code(self) -> &'static str {
        match self {
            Level::Info => "I",
            Level::Warn => "W",
            Level::Error => "E",
        }
    }
    fn from_code(s: &str) -> Level {
        match s {
            "W" => Level::Warn,
            "E" => Level::Error,
            _ => Level::Info,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub ts_ms: u64,
    pub level: Level,
    pub fields: Vec<String>,
}

const MAX_BYTES: u64 = 2 * 1024 * 1024;

/// `$XDG_DATA_HOME/nexdesk/logs` or `~/.local/share/nexdesk/logs`.
pub fn log_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("nexdesk").join("logs"))
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\t' => o.push_str("\\t"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            c if c.is_control() => o.push(' '),
            c => o.push(c),
        }
    }
    o
}

fn unesc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('t') => o.push('\t'),
                Some('n') => o.push('\n'),
                Some('r') => o.push('\r'),
                Some(x) => o.push(x),
                None => o.push('\\'),
            }
        } else {
            o.push(c);
        }
    }
    o
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Append one entry to the default log directory. Never fails the caller.
pub fn append(kind: Kind, level: Level, fields: &[&str]) {
    if let Some(dir) = log_dir() {
        let _ = append_in(&dir, kind, level, fields, now_ms());
    }
}

pub fn append_in(dir: &Path, kind: Kind, level: Level, fields: &[&str], ts_ms: u64) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(kind.file_name());
    if std::fs::metadata(&path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(&path, dir.join(format!("{}.1", kind.file_name())));
    }
    let mut line = format!("{ts_ms}\t{}", level.code());
    for f in fields {
        line.push('\t');
        line.push_str(&esc(f));
    }
    line.push('\n');
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(&path)?.write_all(line.as_bytes())
}

/// Newest-first, at most `max` entries.
pub fn read_tail(kind: Kind, max: usize) -> Vec<Entry> {
    match log_dir() {
        Some(d) => read_tail_in(&d, kind, max),
        None => Vec::new(),
    }
}

pub fn read_tail_in(dir: &Path, kind: Kind, max: usize) -> Vec<Entry> {
    let Ok(text) = std::fs::read_to_string(dir.join(kind.file_name())) else {
        return Vec::new();
    };
    text.lines()
        .rev()
        .filter_map(|l| {
            let mut p = l.split('\t');
            let ts_ms = p.next()?.parse().ok()?;
            let level = Level::from_code(p.next()?);
            Some(Entry { ts_ms, level, fields: p.map(unesc).collect() })
        })
        .take(max)
        .collect()
}

pub fn clear(kind: Kind) {
    if let Some(d) = log_dir() {
        let _ = std::fs::remove_file(d.join(kind.file_name()));
        let _ = std::fs::remove_file(d.join(format!("{}.1", kind.file_name())));
    }
}

// ---- convenience writers used by the engines -------------------------------------------

pub fn connection(level: Level, event: &str, profile: &str, host: &str, user: &str, detail: &str) {
    append(Kind::Connection, level, &[event, profile, host, user, detail]);
}

static CONTEXT_HOST: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Remember which computer this process talks to (used when a writer does not know it).
pub fn set_context_host(host: &str) {
    let _ = CONTEXT_HOST.set(host.to_string());
}

pub fn file(level: Level, direction: &str, items: usize, bytes: u64, status: &str, host: &str) {
    let host = if host.is_empty() { CONTEXT_HOST.get().map(String::as_str).unwrap_or("") } else { host };
    let size = if bytes == 0 { "-".to_string() } else { human_size(bytes) };
    append(Kind::File, level, &[direction, &items.to_string(), &size, status, host]);
}

pub fn alarm(level: Level, title: &str, host: &str, detail: &str) {
    append(Kind::Alarm, level, &[title, host, detail]);
}

pub fn console(level: Level, source: &str, message: &str) {
    append(Kind::Console, level, &[source, message]);
}

pub fn human_size(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}

/// Local time `YYYY-MM-DD HH:MM:SS` (UTC if the platform cannot tell).
pub fn format_time(ts_ms: u64) -> String {
    let secs = (ts_ms / 1000) as i64;
    #[cfg(unix)]
    {
        let t: libc::time_t = secs as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if !unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
            return format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                tm.tm_year + 1900,
                tm.tm_mon + 1,
                tm.tm_mday,
                tm.tm_hour,
                tm.tm_min,
                tm.tm_sec
            );
        }
    }
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[allow(dead_code)]
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---- devices: per-computer summary derived from the connection log ---------------------

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceStats {
    pub last_ts_ms: u64,
    pub sessions: u32,
    pub last_event: String,
    pub last_user: String,
}

/// `entries` is newest-first (as returned by `read_tail`).
pub fn device_stats(entries: &[Entry], host: &str) -> DeviceStats {
    let mut s = DeviceStats::default();
    for e in entries {
        if e.fields.get(2).map(String::as_str) != Some(host) {
            continue;
        }
        let ev = e.fields.first().map(String::as_str).unwrap_or("");
        if s.last_ts_ms == 0 {
            s.last_ts_ms = e.ts_ms;
            s.last_event = ev.to_string();
            s.last_user = e.fields.get(3).cloned().unwrap_or_default();
        }
        if ev == "Connected" {
            s.sessions += 1;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nexdesk-logs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn roundtrip_and_escaping() {
        let d = tmp("rt");
        append_in(&d, Kind::Console, Level::Warn, &["engine", "a\tb\nc\\d"], 1000).unwrap();
        append_in(&d, Kind::Console, Level::Info, &["engine", "second"], 2000).unwrap();
        let v = read_tail_in(&d, Kind::Console, 10);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].fields[1], "second"); // newest first
        assert_eq!(v[1].fields[1], "a\tb\nc\\d");
        assert_eq!(v[1].level, Level::Warn);
        assert_eq!(read_tail_in(&d, Kind::Console, 1).len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rotates_when_large() {
        let d = tmp("rot");
        let big = "x".repeat(4096);
        for i in 0..600 {
            append_in(&d, Kind::File, Level::Info, &[&big], i).unwrap();
        }
        assert!(d.join("file.log.1").exists());
        assert!(std::fs::metadata(d.join("file.log")).unwrap().len() < 3 * 1024 * 1024);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn garbage_lines_are_skipped() {
        let d = tmp("bad");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("alarm.log"), "junk\n\n5\tE\tTitle\thost\tdetail\n").unwrap();
        let v = read_tail_in(&d, Kind::Alarm, 10);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].level, Level::Error);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sizes_and_dates() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
    }

    #[test]
    fn device_summary() {
        let mk = |ts, ev: &str, host: &str| Entry {
            ts_ms: ts,
            level: Level::Info,
            fields: vec![ev.into(), "p".into(), host.into(), "bob".into(), String::new()],
        };
        let entries = vec![mk(30, "Disconnected", "h1"), mk(20, "Connected", "h1"), mk(10, "Connected", "h2"), mk(5, "Connected", "h1")];
        let s = device_stats(&entries, "h1");
        assert_eq!((s.last_ts_ms, s.sessions, s.last_event.as_str()), (30, 2, "Disconnected"));
        assert_eq!(device_stats(&entries, "nope").sessions, 0);
    }
}
