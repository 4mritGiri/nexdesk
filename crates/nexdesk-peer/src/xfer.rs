//! File and folder transfer over the encrypted session.
//!
//! The viewer offers a batch of files; the agent's user must say yes; then the files are streamed in
//! small chunks with a sliding window (the receiver acknowledges, so memory stays small). The receiver
//! never trusts the sender: every path is checked, sizes are enforced exactly, files are written under
//! one folder, never overwrite anything, and appear under their final name only when complete.
//! File contents and names are never logged.
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use crate::wire::{Msg, MAX_BATCH_BYTES, MAX_CHUNK, MAX_FILES, MAX_FILE_SIZE, MAX_PATH};

/// Bytes the sender may have in flight before it waits for an acknowledgement.
const WINDOW: u64 = 4 * 1024 * 1024;
/// The receiver acknowledges at least this often.
const ACK_EVERY: u64 = 1024 * 1024;

/// A relative path from the other side, checked: '/' separated, no `.`/`..`, no empty parts,
/// no backslashes or control characters, parts at most 255 bytes.
pub fn sanitize_rel(path: &str) -> Result<PathBuf, &'static str> {
    if path.is_empty() || path.len() > MAX_PATH {
        return Err("bad file name length");
    }
    let mut out = PathBuf::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.len() > 255 {
            return Err("unsafe file name");
        }
        if part.chars().any(|c| c.is_control() || c == '\\') {
            return Err("unsafe file name");
        }
        out.push(part);
    }
    Ok(out)
}

/// The folder where received files go: `Downloads/NexDesk` (from `~/.config/user-dirs.dirs` if set).
pub fn download_dir() -> Option<PathBuf> {
    let home = crate::paths::home_dir()?;
    let dirs = crate::paths::user_dirs_file();
    let mut base = home.join("Downloads");
    if let Some(text) = dirs {
        for line in text.lines() {
            if let Some(v) = line.trim().strip_prefix("XDG_DOWNLOAD_DIR=") {
                let v = v.trim().trim_matches('"');
                if let Some(rest) = v.strip_prefix("$HOME") {
                    let rest = rest.trim_start_matches('/');
                    if !rest.contains("..") {
                        base = if rest.is_empty() {
                            home.clone()
                        } else {
                            home.join(rest)
                        };
                    }
                } else if v.starts_with('/') && !v.contains("..") {
                    base = PathBuf::from(v);
                }
            }
        }
    }
    Some(base.join("NexDesk"))
}

// ------------------------------------------------------------------------------------------- receiving

struct Current {
    id: u32,
    file: File,
    part: PathBuf,
    dest: PathBuf,
    size: u64,
    got: u64,
    acked: u64,
}

#[derive(Default)]
struct Batch {
    id: u32,
    count: u32,
    total: u64,
    started: u32,
    declared: u64,
}

/// Receiving side of a transfer. One batch at a time.
pub struct Receiver {
    root: PathBuf,
    offered: Option<Batch>,
    batch: Option<Batch>,
    cur: Option<Current>,
}

impl Receiver {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            offered: None,
            batch: None,
            cur: None,
        }
    }

    /// A new offer. Refused while another transfer is running.
    pub fn offer(&mut self, batch: u32, count: u32, total: u64) -> Result<(), &'static str> {
        if self.batch.is_some() || self.offered.is_some() {
            return Err("another transfer is already running");
        }
        if count == 0 || count > MAX_FILES || total > MAX_BATCH_BYTES {
            return Err("transfer too large");
        }
        self.offered = Some(Batch {
            id: batch,
            count,
            total,
            ..Batch::default()
        });
        Ok(())
    }

    /// The user's answer to the last offer.
    pub fn answer(&mut self, batch: u32, accept: bool) {
        if let Some(o) = self.offered.take() {
            if accept && o.id == batch {
                self.batch = Some(o);
            }
        }
    }

    pub fn start(&mut self, batch: u32, id: u32, size: u64, path: &str) -> Result<(), String> {
        let b = self
            .batch
            .as_mut()
            .filter(|b| b.id == batch)
            .ok_or("files that were not accepted")?;
        if self.cur.is_some() {
            return Err("a file is already open".into());
        }
        b.started += 1;
        b.declared = b.declared.saturating_add(size);
        if b.started > b.count || b.declared > b.total || size > MAX_FILE_SIZE {
            return Err("more data than was offered".into());
        }
        let rel = sanitize_rel(path).map_err(str::to_string)?;
        let dest = self.root.join(&rel);
        let dir = dest.parent().ok_or("bad file name")?.to_path_buf();
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create the folder: {e}"))?;
        // never write through a symlink that points out of the download folder
        let root_real = std::fs::canonicalize(&self.root).map_err(|e| e.to_string())?;
        let dir_real = std::fs::canonicalize(&dir).map_err(|e| e.to_string())?;
        if !dir_real.starts_with(&root_real) {
            return Err("unsafe folder".into());
        }
        let name = dest
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("bad file name")?;
        let part = dir.join(format!(".{name}.{id}.nexdesk-part"));
        let mut oo = OpenOptions::new();
        oo.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            oo.mode(0o600);
        }
        let file = oo
            .open(&part)
            .map_err(|e| format!("cannot create the file: {e}"))?;
        self.cur = Some(Current {
            id,
            file,
            part,
            dest,
            size,
            got: 0,
            acked: 0,
        });
        Ok(())
    }

    /// Store a chunk. `Some(bytes)` means an acknowledgement should be sent.
    pub fn chunk(&mut self, id: u32, data: &[u8]) -> Result<Option<u64>, String> {
        let c = self
            .cur
            .as_mut()
            .filter(|c| c.id == id)
            .ok_or("data for a file that is not open")?;
        if data.len() > MAX_CHUNK || c.got + data.len() as u64 > c.size {
            return Err("more data than announced".into());
        }
        c.file
            .write_all(data)
            .map_err(|e| format!("cannot write: {e}"))?;
        c.got += data.len() as u64;
        if c.got - c.acked >= ACK_EVERY {
            c.acked = c.got;
            return Ok(Some(c.got));
        }
        Ok(None)
    }

    /// Finish the file: size must match exactly. Returns where it was stored and the final byte count.
    pub fn end(&mut self, id: u32) -> Result<(PathBuf, u64), String> {
        let c = self
            .cur
            .take()
            .filter(|c| c.id == id)
            .ok_or("end of a file that is not open")?;
        if c.got != c.size {
            let _ = std::fs::remove_file(&c.part);
            return Err("the file ended early".into());
        }
        c.file
            .sync_all()
            .map_err(|e| format!("cannot write: {e}"))?;
        drop(c.file);
        let dest =
            unique_link(&c.part, &c.dest).map_err(|e| format!("cannot save the file: {e}"))?;
        let _ = std::fs::remove_file(&c.part);
        if self
            .batch
            .as_ref()
            .map(|b| b.started >= b.count)
            .unwrap_or(false)
        {
            self.batch = None;
        }
        Ok((dest, c.size))
    }

    /// Drop a half written file (connection lost, user cancelled).
    pub fn abort(&mut self) {
        if let Some(c) = self.cur.take() {
            drop(c.file);
            let _ = std::fs::remove_file(&c.part);
        }
        self.batch = None;
        self.offered = None;
    }

    pub fn busy(&self) -> bool {
        self.batch.is_some() || self.offered.is_some()
    }
}

/// Give `part` its final name without ever replacing an existing file: `name`, `name (1)`, ...
fn unique_link(part: &Path, dest: &Path) -> std::io::Result<PathBuf> {
    let stem = dest
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = dest
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let dir = dest.parent().unwrap_or_else(|| Path::new("."));
    let mut candidate = dest.to_path_buf();
    for i in 1..1000 {
        match std::fs::hard_link(part, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                candidate = dir.join(format!("{stem} ({i}){ext}"));
            }
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "too many files with that name",
    ))
}

// -------------------------------------------------------------------------------------------- sending

#[derive(Debug, Clone)]
pub struct Item {
    pub abs: PathBuf,
    /// '/' separated path the receiver will see.
    pub rel: String,
    pub size: u64,
}

/// What the sender hears back from the other side.
#[derive(Debug, Clone)]
pub enum Event {
    Answer(bool),
    Ack { id: u32, bytes: u64 },
    Abort(String),
}

/// Expand dropped paths into files: a folder keeps its name and structure. Symlinks and special
/// files are skipped.
pub fn collect(paths: &[PathBuf]) -> Result<Vec<Item>, String> {
    let mut items = Vec::new();
    let mut total = 0u64;
    let mut stack: Vec<(PathBuf, String)> = Vec::new();
    for p in paths {
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("a dropped item has no usable name")?
            .to_string();
        stack.push((p.clone(), name));
    }
    while let Some((path, rel)) = stack.pop() {
        let md = std::fs::symlink_metadata(&path).map_err(|e| format!("cannot read {rel}: {e}"))?;
        if md.file_type().is_symlink() {
            continue;
        }
        if md.is_dir() {
            let mut kids: Vec<_> = std::fs::read_dir(&path)
                .map_err(|e| format!("cannot read {rel}: {e}"))?
                .flatten()
                .collect();
            kids.sort_by_key(|k| k.file_name());
            for k in kids.into_iter().rev() {
                if let Some(n) = k.file_name().to_str() {
                    stack.push((k.path(), format!("{rel}/{n}")));
                }
            }
        } else if md.is_file() {
            if md.len() > MAX_FILE_SIZE {
                return Err(format!(
                    "{rel} is larger than the {} GiB limit",
                    MAX_FILE_SIZE >> 30
                ));
            }
            if sanitize_rel(&rel).is_err() {
                continue; // a name the other side would refuse anyway
            }
            total = total.saturating_add(md.len());
            items.push(Item {
                abs: path,
                rel,
                size: md.len(),
            });
            if items.len() as u32 > MAX_FILES {
                return Err(format!("more than {MAX_FILES} files"));
            }
            if total > MAX_BATCH_BYTES {
                return Err("more than 64 GiB in one transfer".into());
            }
        }
    }
    if items.is_empty() {
        return Err("nothing to send".into());
    }
    Ok(items)
}

/// Send `items` as batch `batch`: offer, wait for the answer, stream with a window.
/// `progress(done, total)` is called as bytes are sent. Returns the number of files sent.
pub fn send(
    batch: u32,
    items: &[Item],
    out: &mpsc::Sender<Msg>,
    events: &mpsc::Receiver<Event>,
    progress: &dyn Fn(u64, u64),
    cancel: &AtomicBool,
) -> Result<usize, String> {
    let total: u64 = items.iter().map(|i| i.size).sum();
    let first = items[0].rel.clone();
    out.send(Msg::FilesOffer {
        batch,
        count: items.len() as u32,
        total,
        first,
    })
    .map_err(|_| "disconnected")?;
    match events.recv_timeout(Duration::from_secs(120)) {
        Ok(Event::Answer(true)) => {}
        Ok(Event::Answer(false)) => return Err("the other side said no".into()),
        Ok(Event::Abort(r)) => return Err(r),
        _ => return Err("no answer from the other side".into()),
    }
    let mut done = 0u64;
    let mut buf = vec![0u8; MAX_CHUNK];
    for (n, it) in items.iter().enumerate() {
        let id = n as u32;
        let mut f = File::open(&it.abs).map_err(|e| format!("cannot open {}: {e}", it.rel))?;
        out.send(Msg::FileStart {
            batch,
            id,
            size: it.size,
            path: it.rel.clone(),
        })
        .map_err(|_| "disconnected")?;
        let (mut sent, mut acked) = (0u64, 0u64);
        while sent < it.size {
            if cancel.load(Ordering::Relaxed) {
                let _ = out.send(Msg::FileAbort {
                    batch,
                    reason: "cancelled".into(),
                });
                return Err("cancelled".into());
            }
            while sent - acked >= WINDOW {
                match events.recv_timeout(Duration::from_secs(60)) {
                    Ok(Event::Ack { id: a, bytes }) if a == id => acked = acked.max(bytes),
                    Ok(Event::Abort(r)) => return Err(r),
                    Ok(_) => {}
                    Err(_) => return Err("the other side stopped answering".into()),
                }
            }
            let want = ((it.size - sent) as usize).min(MAX_CHUNK);
            f.read_exact(&mut buf[..want])
                .map_err(|e| format!("cannot read {}: {e}", it.rel))?;
            out.send(Msg::FileChunk {
                id,
                data: buf[..want].to_vec(),
            })
            .map_err(|_| "disconnected")?;
            sent += want as u64;
            done += want as u64;
            progress(done, total);
        }
        out.send(Msg::FileEnd { id }).map_err(|_| "disconnected")?;
        // wait for the receiver to confirm the file is stored
        loop {
            match events.recv_timeout(Duration::from_secs(60)) {
                Ok(Event::Ack { id: a, bytes }) if a == id && bytes >= it.size => break,
                Ok(Event::Abort(r)) => return Err(r),
                Ok(_) => {}
                Err(_) => return Err("the other side stopped answering".into()),
            }
        }
    }
    Ok(items.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    fn temp(name: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "nexdesk-xfer-{}-{}-{name}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn names_from_the_network_are_checked() {
        assert_eq!(
            sanitize_rel("a/b/c.txt").unwrap(),
            PathBuf::from("a/b/c.txt")
        );
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../../x",
            "a//b",
            "a/./b",
            "a\\b",
            "a/\u{0}b",
            "a/\nb",
            "./",
            "a/",
        ] {
            assert!(sanitize_rel(bad).is_err(), "{bad:?}");
        }
        assert!(sanitize_rel(&"x".repeat(256)).is_err());
        assert!(sanitize_rel(&format!("{}/y", "p".repeat(300))).is_err());
    }

    #[test]
    fn receiver_enforces_everything_it_was_promised() {
        let root = temp("recv");
        let mut r = Receiver::new(root.clone());
        assert!(r.start(1, 0, 3, "a.txt").is_err(), "nothing accepted yet");
        r.offer(1, 1, 3).unwrap();
        assert!(r.offer(2, 1, 3).is_err(), "one at a time");
        r.answer(1, true);
        assert!(r.start(1, 0, 4, "a.txt").is_err(), "more than offered");
        // a failed start still counted; begin again with a fresh batch
        r.abort();
        r.offer(2, 2, 10).unwrap();
        r.answer(2, true);
        assert!(r.start(2, 0, 3, "../evil").is_err());
        r.abort();
        r.offer(3, 1, 3).unwrap();
        r.answer(3, true);
        r.start(3, 0, 3, "a.txt").unwrap();
        assert!(r.chunk(0, b"abcd").is_err(), "more than announced");
        r.chunk(0, b"ab").unwrap();
        assert!(r.end(0).is_err(), "ended early");
        assert!(
            !root.join("a.txt").exists(),
            "an incomplete file never appears"
        );
        assert!(
            std::fs::read_dir(&root)
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().contains("nexdesk-part")),
            "partial file removed"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn declined_offers_write_nothing_and_existing_files_survive() {
        let root = temp("decline");
        let mut r = Receiver::new(root.clone());
        r.offer(1, 1, 3).unwrap();
        r.answer(1, false);
        assert!(r.start(1, 0, 3, "a.txt").is_err());
        std::fs::write(root.join("a.txt"), b"mine").unwrap();
        r.offer(2, 1, 3).unwrap();
        r.answer(2, true);
        r.start(2, 0, 3, "a.txt").unwrap();
        r.chunk(0, b"new").unwrap();
        let (p, _) = r.end(0).unwrap();
        assert_eq!(p, root.join("a (1).txt"));
        assert_eq!(
            std::fs::read(root.join("a.txt")).unwrap(),
            b"mine",
            "nothing is overwritten"
        );
        assert_eq!(std::fs::read(&p).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_travels_end_to_end() {
        let src = temp("src");
        std::fs::create_dir_all(src.join("proj/sub")).unwrap();
        std::fs::write(src.join("proj/a.txt"), b"hello").unwrap();
        let big: Vec<u8> = (0..(5 * 1024 * 1024 + 123))
            .map(|i| (i % 251) as u8)
            .collect();
        std::fs::write(src.join("proj/sub/big.bin"), &big).unwrap();
        std::fs::write(src.join("proj/empty"), b"").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/passwd", src.join("proj/link")).unwrap();
        let items = collect(&[src.join("proj")]).unwrap();
        assert_eq!(items.len(), 3, "the symlink is skipped");
        assert!(items.iter().all(|i| i.rel.starts_with("proj/")));

        let dst = temp("dst");
        let (out_tx, out_rx) = mpsc::channel::<Msg>();
        let (ev_tx, ev_rx) = mpsc::channel::<Event>();
        let cancel = Arc::new(AtomicBool::new(false));
        let sender = {
            let (items, cancel) = (items.clone(), cancel.clone());
            std::thread::spawn(move || send(9, &items, &out_tx, &ev_rx, &|_, _| {}, &cancel))
        };
        // the "agent" side
        let mut rx = Receiver::new(dst.clone());
        let mut stored = 0;
        while let Ok(m) = out_rx.recv() {
            match m {
                Msg::FilesOffer {
                    batch,
                    count,
                    total,
                    ..
                } => {
                    rx.offer(batch, count, total).unwrap();
                    rx.answer(batch, true);
                    ev_tx.send(Event::Answer(true)).unwrap();
                }
                Msg::FileStart {
                    batch,
                    id,
                    size,
                    path,
                } => rx.start(batch, id, size, &path).unwrap(),
                Msg::FileChunk { id, data } => {
                    if let Some(b) = rx.chunk(id, &data).unwrap() {
                        ev_tx.send(Event::Ack { id, bytes: b }).unwrap();
                    }
                }
                Msg::FileEnd { id } => {
                    let (_, n) = rx.end(id).unwrap();
                    stored += 1;
                    ev_tx.send(Event::Ack { id, bytes: n }).unwrap();
                    if stored == 3 {
                        break;
                    }
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(sender.join().unwrap().unwrap(), 3);
        assert_eq!(std::fs::read(dst.join("proj/a.txt")).unwrap(), b"hello");
        assert_eq!(std::fs::read(dst.join("proj/sub/big.bin")).unwrap(), big);
        assert_eq!(std::fs::read(dst.join("proj/empty")).unwrap().len(), 0);
        assert!(!dst.join("proj/link").exists());
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    use std::sync::Arc;

    #[test]
    fn collect_refuses_nothing_and_missing() {
        assert!(collect(&[]).is_err());
        assert!(collect(&[PathBuf::from("/definitely/not/here")]).is_err());
    }
}
