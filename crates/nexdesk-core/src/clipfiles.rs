//! Linux-side helpers for Windows -> Linux *file* clipboard (CLIPRDR file transfer).
//!
//! Linux has no virtual-file clipboard, so the client downloads the remote files
//! (`FileContentsRequest`) into a private staging directory and then publishes
//! real `file://` paths on the local clipboard (`text/uri-list` and GNOME's
//! `x-special/gnome-copied-files`). This module holds the security-critical and
//! format-critical parts: path sanitising, URI building, and the staging dir.
use std::io;
use std::path::{Path, PathBuf};

/// One entry from the remote `FileGroupDescriptorW`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Path as sent by the server (Windows separators, relative to the copied root).
    pub rel_path: String,
    pub size: u64,
    pub is_dir: bool,
}

/// Turn a server-supplied name into a safe relative path using `/` separators.
/// Returns `None` for anything that could escape the staging directory
/// (`..`, drive letters / alternate data streams via `:`, NUL) or is empty.
/// The server is untrusted: this blocks classic path-traversal attacks.
pub fn sanitize_relative_path(name: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for comp in name.split(|c| c == '\\' || c == '/') {
        match comp {
            "" | "." => continue,
            ".." => return None,
            c if c.contains('\0') || c.contains(':') => return None,
            c => parts.push(c),
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Percent-encode a path for use inside a `file://` URI (keeps `/`).
pub fn percent_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for &b in path.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// `file:///abs/path` for an absolute path; `None` if the path is relative.
pub fn file_uri(path: &Path) -> Option<String> {
    if !path.is_absolute() {
        return None;
    }
    Some(format!(
        "file://{}",
        percent_encode_path(&path.to_string_lossy())
    ))
}

/// Payload for the `text/uri-list` clipboard target (CRLF terminated lines).
pub fn uri_list(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .filter_map(|p| file_uri(p))
        .map(|u| format!("{u}\r\n"))
        .collect()
}

/// Payload for GNOME/Nautilus `x-special/gnome-copied-files` (`copy`/`cut`, then URIs).
pub fn gnome_copied_files(paths: &[PathBuf], cut: bool) -> String {
    let mut s = String::from(if cut { "cut" } else { "copy" });
    for u in paths.iter().filter_map(|p| file_uri(p)) {
        s.push('\n');
        s.push_str(&u);
    }
    s
}

/// A private per-session directory (mode 0700 on Unix) that receives pasted files.
#[derive(Debug)]
pub struct StagingDir {
    root: PathBuf,
}

impl StagingDir {
    /// Creates `<base>/<session_id>`. `session_id` must be a single safe component.
    pub fn create(base: &Path, session_id: &str) -> io::Result<Self> {
        let bad = || io::Error::new(io::ErrorKind::InvalidInput, "invalid session id");
        let id = sanitize_relative_path(session_id).ok_or_else(bad)?;
        if id.contains('/') {
            return Err(bad());
        }
        let root = base.join(id);
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            b.mode(0o700);
        }
        b.create(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a server-supplied path inside the staging root (never outside it).
    pub fn resolve(&self, rel: &str) -> Option<PathBuf> {
        Some(self.root.join(sanitize_relative_path(rel)?))
    }

    /// Create the directory (or the parent directories of the file) for `entry`
    /// and return the destination path the caller should write bytes to.
    pub fn prepare_entry(&self, entry: &FileEntry) -> io::Result<PathBuf> {
        let path = self.resolve(&entry.rel_path).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "unsafe path from server")
        })?;
        if entry.is_dir {
            std::fs::create_dir_all(&path)?;
        } else if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(path)
    }

    /// Paths to publish on the clipboard: only the top-level files/folders
    /// (the file manager copies folders recursively).
    pub fn top_level_paths(&self, entries: &[FileEntry]) -> Vec<PathBuf> {
        let mut seen: Vec<String> = Vec::new();
        for e in entries {
            if let Some(clean) = sanitize_relative_path(&e.rel_path) {
                let first = clean.split('/').next().unwrap_or("").to_string();
                if !first.is_empty() && !seen.contains(&first) {
                    seen.push(first);
                }
            }
        }
        seen.into_iter().map(|n| self.root.join(n)).collect()
    }

    pub fn cleanup(self) -> io::Result<()> {
        std::fs::remove_dir_all(self.root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_accepts_normal_and_converts_separators() {
        assert_eq!(
            sanitize_relative_path("a\\b\\c.txt").as_deref(),
            Some("a/b/c.txt")
        );
        assert_eq!(
            sanitize_relative_path("\\\\lead\\x").as_deref(),
            Some("lead/x")
        );
        assert_eq!(sanitize_relative_path("./x/./y").as_deref(), Some("x/y"));
    }

    #[test]
    fn sanitize_blocks_traversal_and_oddities() {
        assert_eq!(sanitize_relative_path("..\\..\\etc\\passwd"), None);
        assert_eq!(sanitize_relative_path("a/../../b"), None);
        assert_eq!(sanitize_relative_path("C:\\Windows\\x"), None);
        assert_eq!(sanitize_relative_path("file.txt:stream"), None);
        assert_eq!(sanitize_relative_path("a\0b"), None);
        assert_eq!(sanitize_relative_path(""), None);
        assert_eq!(sanitize_relative_path("///"), None);
    }

    #[test]
    fn uri_encoding() {
        assert_eq!(
            percent_encode_path("/tmp/my file#1.txt"),
            "/tmp/my%20file%231.txt"
        );
        assert_eq!(percent_encode_path("/tmp/é"), "/tmp/%C3%A9");
        assert_eq!(file_uri(Path::new("relative")), None);
        assert_eq!(
            file_uri(Path::new("/tmp/a b")).as_deref(),
            Some("file:///tmp/a%20b")
        );
    }

    #[test]
    fn clipboard_payloads() {
        let p = vec![PathBuf::from("/tmp/a b"), PathBuf::from("/tmp/c")];
        assert_eq!(uri_list(&p), "file:///tmp/a%20b\r\nfile:///tmp/c\r\n");
        assert_eq!(
            gnome_copied_files(&p, false),
            "copy\nfile:///tmp/a%20b\nfile:///tmp/c"
        );
        assert_eq!(gnome_copied_files(&p[..1], true), "cut\nfile:///tmp/a%20b");
    }

    #[test]
    fn staging_dir_lifecycle_and_confinement() {
        let base = std::env::temp_dir().join(format!("nexdesk-test-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let sd = StagingDir::create(&base, "sess1").unwrap();
        assert!(sd.root().is_dir());

        assert!(StagingDir::create(&base, "../evil").is_err());
        assert!(StagingDir::create(&base, "a/b").is_err());

        let entries = vec![
            FileEntry {
                rel_path: "Docs".into(),
                size: 0,
                is_dir: true,
            },
            FileEntry {
                rel_path: "Docs\\r.txt".into(),
                size: 3,
                is_dir: false,
            },
            FileEntry {
                rel_path: "solo.bin".into(),
                size: 9,
                is_dir: false,
            },
        ];
        for e in &entries {
            let p = sd.prepare_entry(e).unwrap();
            assert!(p.starts_with(sd.root()));
        }
        assert!(sd.root().join("Docs").is_dir());

        let evil = FileEntry {
            rel_path: "..\\..\\x".into(),
            size: 1,
            is_dir: false,
        };
        assert!(sd.prepare_entry(&evil).is_err());

        let top = sd.top_level_paths(&entries);
        assert_eq!(
            top,
            vec![sd.root().join("Docs"), sd.root().join("solo.bin")]
        );

        let root = sd.root().to_path_buf();
        sd.cleanup().unwrap();
        assert!(!root.exists());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
