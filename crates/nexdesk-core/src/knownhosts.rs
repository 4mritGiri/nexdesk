//! Pinned server certificate fingerprints ("known hosts", like SSH).
//!
//! RDP servers almost always present self-signed certificates, so chain validation alone
//! rejects them. Trust-on-first-use pinning gives the same protection as SSH: the first
//! connection records the SHA-256 fingerprint, later connections must present the same one.
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Unknown,
    Match,
    /// A different certificate than the pinned one.
    Mismatch { pinned: String },
}

#[derive(Debug, Default)]
pub struct KnownHosts {
    path: Option<PathBuf>,
    entries: Vec<(String, String)>,
}

/// `host:port` key, lowercased (DNS names are case-insensitive).
pub fn host_key(host: &str, port: u16) -> String {
    format!("{}:{port}", host.trim().to_ascii_lowercase())
}

/// Lowercase hex fingerprint with a `sha256:` prefix.
pub fn fingerprint_string(digest: &[u8]) -> String {
    let mut s = String::from("sha256:");
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// `~/.config/nexdesk/known_hosts` (honours `XDG_CONFIG_HOME`).
pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("nexdesk").join("known_hosts"))
}

impl KnownHosts {
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn load(path: &Path) -> Self {
        let mut k = KnownHosts { path: Some(path.to_path_buf()), entries: Vec::new() };
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let mut it = line.split_whitespace();
                if let (Some(h), Some(f)) = (it.next(), it.next()) {
                    if f.starts_with("sha256:") {
                        k.entries.push((h.to_owned(), f.to_owned()));
                    }
                }
            }
        }
        k
    }

    pub fn lookup(&self, key: &str, fingerprint: &str) -> Lookup {
        match self.entries.iter().find(|(h, _)| h == key) {
            None => Lookup::Unknown,
            Some((_, f)) if f == fingerprint => Lookup::Match,
            Some((_, f)) => Lookup::Mismatch { pinned: f.clone() },
        }
    }

    /// Pin (or re-pin) a host and persist the file (mode 0600).
    pub fn set(&mut self, key: &str, fingerprint: &str) -> io::Result<()> {
        match self.entries.iter_mut().find(|(h, _)| h == key) {
            Some(e) => e.1 = fingerprint.to_owned(),
            None => self.entries.push((key.to_owned(), fingerprint.to_owned())),
        }
        self.save()
    }

    pub fn forget(&mut self, key: &str) -> io::Result<bool> {
        let before = self.entries.len();
        self.entries.retain(|(h, _)| h != key);
        let removed = self.entries.len() != before;
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut text = String::from("# NexDesk pinned RDP server certificates: <host:port> <sha256:fingerprint>\n");
        for (h, f) in &self.entries {
            text.push_str(&format!("{h} {f}\n"));
        }
        let tmp = path.with_extension("tmp");
        {
            use std::io::Write;
            #[cfg(unix)]
            let mut f = {
                use std::os::unix::fs::OpenOptionsExt;
                std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?
            };
            #[cfg(not(unix))]
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(text.as_bytes())?;
        }
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_match_mismatch_and_persist() {
        let dir = std::env::temp_dir().join(format!("nexdesk-kh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("sub").join("known_hosts");
        let mut k = KnownHosts::load(&path);
        let key = host_key(" Server.Example ", 3389);
        assert_eq!(key, "server.example:3389");
        assert_eq!(k.lookup(&key, "sha256:aa"), Lookup::Unknown);
        k.set(&key, "sha256:aa").unwrap();
        assert_eq!(k.lookup(&key, "sha256:aa"), Lookup::Match);
        assert_eq!(k.lookup(&key, "sha256:bb"), Lookup::Mismatch { pinned: "sha256:aa".into() });

        let k2 = KnownHosts::load(&path);
        assert_eq!(k2.lookup(&key, "sha256:aa"), Lookup::Match);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let mut k3 = KnownHosts::load(&path);
        k3.set(&key, "sha256:cc").unwrap();
        assert_eq!(KnownHosts::load(&path).lookup(&key, "sha256:cc"), Lookup::Match);
        assert!(k3.forget(&key).unwrap());
        assert_eq!(KnownHosts::load(&path).lookup(&key, "sha256:cc"), Lookup::Unknown);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn fingerprint_format_and_garbage_lines() {
        assert_eq!(fingerprint_string(&[0x0a, 0xff]), "sha256:0aff");
        let dir = std::env::temp_dir().join(format!("nexdesk-kh2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("kh");
        std::fs::write(&p, "# c\n\nbad\nhost:1 md5:zz\nok:3389 sha256:12\n").unwrap();
        let k = KnownHosts::load(&p);
        assert_eq!(k.lookup("ok:3389", "sha256:12"), Lookup::Match);
        assert_eq!(k.lookup("bad", "x"), Lookup::Unknown);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
