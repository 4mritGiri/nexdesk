//! Local file table for Linux -> Windows file copy (and drag & drop), plus the file server
//! thread that answers `FileContentsRequest`s from it.
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use ironrdp_cliprdr::backend::ClipboardMessage;
use ironrdp_cliprdr::pdu::{
    ClipboardFileAttributes, FileContentsFlags, FileContentsRequest, FileContentsResponse,
    FileDescriptor,
};

use crate::Sink;

/// Limits that keep a hostile/huge selection from exhausting memory or the channel.
pub const MAX_ENTRIES: usize = 100_000;
const MAX_DEPTH: usize = 64;
/// MS-RDPECLIP: wire file name is 260 UTF-16 units including the NUL terminator.
const MAX_WIRE_NAME: usize = 259;

#[derive(Debug, Clone)]
pub struct LocalFile {
    pub path: PathBuf,
    pub size: u64,
    pub is_dir: bool,
}

/// An immutable snapshot: entry `i` corresponds to descriptor `i` sent to the server.
#[derive(Debug, Default)]
pub struct LocalTable {
    pub files: Vec<LocalFile>,
    pub descriptors: Vec<FileDescriptor>,
}

fn filetime(meta: &std::fs::Metadata) -> Option<u64> {
    let d = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some((d.as_secs() + 11_644_473_600) * 10_000_000 + u64::from(d.subsec_nanos()) / 100)
}

impl LocalTable {
    /// Build the table (recursing into directories, never following directory symlinks).
    /// Entries that cannot be represented on the wire are skipped, so indices stay aligned
    /// with what we advertise (the CLIPRDR layer would otherwise drop them silently).
    pub fn build(paths: &[PathBuf]) -> LocalTable {
        let mut t = LocalTable::default();
        for p in paths {
            t.add(p, None, 0);
        }
        t
    }

    fn add(&mut self, path: &Path, rel_dir: Option<&str>, depth: usize) {
        if self.files.len() >= MAX_ENTRIES || depth > MAX_DEPTH {
            return;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
            tracing::debug!("skipping {path:?}: no UTF-8 file name");
            return;
        };
        if name.contains('\\') || name.contains('\0') {
            tracing::debug!("skipping {path:?}: unusable name");
            return;
        }
        let Ok(link_meta) = std::fs::symlink_metadata(path) else {
            return;
        };
        let meta = if link_meta.file_type().is_symlink() {
            match std::fs::metadata(path) {
                Ok(m) if m.is_file() => m, // follow links to files only
                _ => return,
            }
        } else {
            link_meta
        };
        let wire_len = rel_dir.map_or(0, |d| d.chars().count() + 1) + name.chars().count();
        if wire_len > MAX_WIRE_NAME {
            tracing::debug!("skipping {path:?}: name too long for the clipboard protocol");
            return;
        }
        let mut d = FileDescriptor::new(name.clone());
        if let Some(dir) = rel_dir {
            d = d.with_relative_path(dir);
        }
        if let Some(t) = filetime(&meta) {
            d = d.with_last_write_time(t);
        }
        if meta.is_dir() {
            d = d.with_attributes(ClipboardFileAttributes::DIRECTORY);
            self.files.push(LocalFile {
                path: path.to_path_buf(),
                size: 0,
                is_dir: true,
            });
            self.descriptors.push(d);
            let child_rel = match rel_dir {
                Some(dir) => format!("{dir}\\{name}"),
                None => name.clone(),
            };
            let mut children: Vec<PathBuf> = match std::fs::read_dir(path) {
                Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).collect(),
                Err(_) => Vec::new(),
            };
            children.sort();
            for c in children {
                self.add(&c, Some(&child_rel), depth + 1);
            }
        } else if meta.is_file() {
            d = d
                .with_attributes(ClipboardFileAttributes::ARCHIVE)
                .with_file_size(meta.len());
            self.files.push(LocalFile {
                path: path.to_path_buf(),
                size: meta.len(),
                is_dir: false,
            });
            self.descriptors.push(d);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

pub struct FileJob {
    pub request: FileContentsRequest,
    pub table: Arc<LocalTable>,
}

/// Spawns the thread that reads local files for the server. Dropping the sender stops it.
pub fn spawn_file_server(sink: Sink) -> Sender<FileJob> {
    let (tx, rx) = channel::<FileJob>();
    std::thread::Builder::new()
        .name("nexdesk-clip-files".into())
        .spawn(move || {
            let mut open: Option<(PathBuf, File)> = None;
            while let Ok(job) = rx.recv() {
                let resp = serve(&job, &mut open);
                sink(ClipboardMessage::SendFileContentsResponse(resp));
            }
        })
        .ok();
    tx
}

fn serve(job: &FileJob, open: &mut Option<(PathBuf, File)>) -> FileContentsResponse<'static> {
    let r = &job.request;
    let err = || FileContentsResponse::new_error(r.stream_id);
    let Some(entry) = usize::try_from(r.index)
        .ok()
        .and_then(|i| job.table.files.get(i))
    else {
        return err();
    };
    if entry.is_dir {
        return err();
    }
    if r.flags.contains(FileContentsFlags::SIZE) {
        // Re-stat: the file may have grown since it was copied; the protocol wants the real size.
        let size = std::fs::metadata(&entry.path)
            .map(|m| m.len())
            .unwrap_or(entry.size);
        return FileContentsResponse::new_size_response(r.stream_id, size);
    }
    // RANGE
    const MAX_CHUNK: u32 = 4 * 1024 * 1024;
    let want = r.requested_size.min(MAX_CHUNK) as usize;
    if open.as_ref().map(|(p, _)| p != &entry.path).unwrap_or(true) {
        match File::open(&entry.path) {
            Ok(f) => *open = Some((entry.path.clone(), f)),
            Err(e) => {
                tracing::debug!("cannot open {:?}: {e}", entry.path);
                *open = None;
                return err();
            }
        }
    }
    let Some((_, f)) = open.as_mut() else {
        return err();
    };
    if f.seek(SeekFrom::Start(r.position)).is_err() {
        return err();
    }
    let mut buf = vec![0u8; want];
    let mut got = 0;
    while got < want {
        match f.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(_) => return err(),
        }
    }
    buf.truncate(got);
    FileContentsResponse::new_data_response(r.stream_id, buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nexdesk-files-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn builds_descriptors_with_relative_paths() {
        let root = tmpdir("tbl");
        std::fs::create_dir_all(root.join("proj/sub")).unwrap();
        std::fs::write(root.join("proj/a.txt"), b"hello").unwrap();
        std::fs::write(root.join("proj/sub/b.bin"), vec![7u8; 10]).unwrap();
        std::fs::write(root.join("solo.txt"), b"x").unwrap();
        let t = LocalTable::build(&[root.join("proj"), root.join("solo.txt")]);
        let names: Vec<(String, Option<String>)> = t
            .descriptors
            .iter()
            .map(|d| (d.name.clone(), d.relative_path.clone()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("proj".into(), None),
                ("a.txt".into(), Some("proj".into())),
                ("sub".into(), Some("proj".into())),
                ("b.bin".into(), Some("proj\\sub".into())),
                ("solo.txt".into(), None),
            ]
        );
        assert!(t.files[0].is_dir && !t.files[1].is_dir);
        assert_eq!(t.descriptors[1].file_size, Some(5));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn serves_size_and_ranges() {
        let root = tmpdir("srv");
        std::fs::write(root.join("f"), b"0123456789").unwrap();
        let table = Arc::new(LocalTable::build(&[root.join("f")]));
        let mut open = None;
        let mk = |flags, pos, n| FileJob {
            request: FileContentsRequest {
                stream_id: 9,
                index: 0,
                flags,
                position: pos,
                requested_size: n,
                data_id: None,
            },
            table: table.clone(),
        };
        let r = serve(&mk(FileContentsFlags::SIZE, 0, 8), &mut open);
        assert_eq!(r.data_as_size().unwrap(), 10);
        let r = serve(&mk(FileContentsFlags::RANGE, 3, 4), &mut open);
        assert_eq!(r.data(), b"3456");
        let r = serve(&mk(FileContentsFlags::RANGE, 8, 100), &mut open);
        assert_eq!(r.data(), b"89");
        let mut bad = mk(FileContentsFlags::RANGE, 0, 1);
        bad.request.index = 5;
        assert!(serve(&bad, &mut open).is_error());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skips_overlong_names() {
        let root = tmpdir("long");
        // A 200-char folder holding a 100-char file: 301 chars on the wire, over the 259 limit.
        let dir = root.join("d".repeat(200));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f".repeat(100)), b"1").unwrap();
        std::fs::write(root.join("ok"), b"1").unwrap();
        let t = LocalTable::build(&[dir, root.join("ok")]);
        // the folder entry itself and "ok" survive, the long child does not
        assert_eq!(t.files.len(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }
}
