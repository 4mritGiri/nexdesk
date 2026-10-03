//! Saved connections, stored as standard `.rdp` files (one per connection),
//! so they are also readable by mstsc/Remmina/FreeRDP. Passwords are NEVER stored.
use std::io;
use std::path::{Path, PathBuf};

use crate::rdpfile::RdpFile;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub host: String,
    pub user: String,
    pub domain: String,
    pub width: u16,
    pub height: u16,
    pub clipboard: bool,
}

impl Profile {
    /// Stable, filesystem-safe identity used by the session and credential layers.
    pub fn profile_id(&self) -> String {
        file_stem_for(&self.name).unwrap_or_else(|| "connection".into()).to_ascii_lowercase()
    }
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: String::new(),
            user: String::new(),
            domain: String::new(),
            width: 1920,
            height: 1080,
            clipboard: true,
        }
    }
}

impl Profile {
    /// Build from a parsed `.rdp` file; `name` is usually the file stem.
    pub fn from_rdp(name: &str, f: &RdpFile) -> Self {
        let (width, height) = f.desktop_size().unwrap_or((1920, 1080));
        Self {
            name: name.to_string(),
            host: f.full_address().unwrap_or("").to_string(),
            user: f.username().unwrap_or("").to_string(),
            domain: f.domain().unwrap_or("").to_string(),
            width,
            height,
            // mstsc's own key for clipboard redirection is "redirectclipboard"
            clipboard: f
                .get_int("redirectclipboard")
                .or_else(|| f.get_int("myrdp clipboard"))
                .map(|v| v != 0)
                .unwrap_or(true),
        }
    }

    pub fn to_rdp_text(&self) -> String {
        let clean = |s: &str| s.replace(['\r', '\n'], " ");
        let mut s = String::new();
        s.push_str(&format!("full address:s:{}\r\n", clean(&self.host)));
        if !self.user.is_empty() {
            s.push_str(&format!("username:s:{}\r\n", clean(&self.user)));
        }
        if !self.domain.is_empty() {
            s.push_str(&format!("domain:s:{}\r\n", clean(&self.domain)));
        }
        s.push_str(&format!("desktopwidth:i:{}\r\n", self.width));
        s.push_str(&format!("desktopheight:i:{}\r\n", self.height));
        s.push_str(&format!("redirectclipboard:i:{}\r\n", self.clipboard as u8));
        s
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.host.trim().is_empty() {
            return Err("Computer (host) is required");
        }
        if self.user.trim().is_empty() {
            return Err("User name is required");
        }
        if self.width < 200 || self.height < 200 {
            return Err("Desktop size is too small (minimum 200x200)");
        }
        Ok(())
    }
}

/// Make a user-typed name safe as a file stem.
pub fn file_stem_for(name: &str) -> Option<String> {
    let s: String = name
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') { c } else { '_' })
        .collect();
    let s = s.trim_matches(|c| c == '.' || c == ' ').to_string();
    if s.is_empty() || s.len() > 100 {
        None
    } else {
        Some(s)
    }
}

/// `<config>/nexdesk/connections`, following each OS convention (std only).
pub fn default_store_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA")?)
    } else if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support")
    } else {
        match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            Some(x) => PathBuf::from(x),
            None => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
        }
    };
    Some(base.join("nexdesk").join("connections"))
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn save(&self, p: &Profile) -> io::Result<()> {
        let stem = file_stem_for(&p.name)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid connection name"))?;
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(self.dir.join(format!("{stem}.rdp")), p.to_rdp_text())
    }

    pub fn delete(&self, name: &str) -> io::Result<()> {
        let stem = file_stem_for(name)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid connection name"))?;
        std::fs::remove_file(self.dir.join(format!("{stem}.rdp")))
    }

    /// All saved profiles, sorted by name (case-insensitive). Unreadable files are skipped.
    pub fn list(&self) -> Vec<Profile> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return out };
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("rdp") {
                continue;
            }
            let (Some(stem), Ok(text)) = (path.file_stem().and_then(|s| s.to_str()), std::fs::read_to_string(&path))
            else {
                continue;
            };
            out.push(Profile::from_rdp(stem, &RdpFile::parse(&text)));
        }
        out.sort_by_key(|p| p.name.to_lowercase());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Profile {
        Profile {
            name: "Jump Server".into(),
            host: "jump.corp:3390".into(),
            user: "alice".into(),
            domain: "CORP".into(),
            width: 1600,
            height: 900,
            clipboard: false,
        }
    }

    #[test]
    fn roundtrip_through_rdp_text() {
        let p = sample();
        let back = Profile::from_rdp("Jump Server", &RdpFile::parse(&p.to_rdp_text()));
        assert_eq!(p, back);
    }

    #[test]
    fn newlines_cannot_inject_extra_rdp_keys() {
        let mut p = sample();
        p.host = "h\r\nusername:s:evil".into();
        let f = RdpFile::parse(&p.to_rdp_text());
        assert_eq!(f.username(), Some("alice"));
    }

    #[test]
    fn validation() {
        assert!(sample().validate().is_ok());
        let mut p = sample();
        p.host = "  ".into();
        assert!(p.validate().is_err());
        p = sample();
        p.user.clear();
        assert!(p.validate().is_err());
        p = sample();
        p.width = 10;
        assert!(p.validate().is_err());
    }

    #[test]
    fn file_stems_are_safe() {
        assert_eq!(file_stem_for("My Server-1").as_deref(), Some("My Server-1"));
        assert_eq!(file_stem_for("../../etc/passwd").as_deref(), Some("_.._etc_passwd"));
        assert_eq!(file_stem_for("a/b\\c").as_deref(), Some("a_b_c"));
        assert_eq!(file_stem_for("   "), None);
        assert_eq!(file_stem_for("..."), None);
    }

    #[test]
    fn store_save_list_delete() {
        let dir = std::env::temp_dir().join(format!("nexdesk-store-{}", std::process::id()));
        let store = Store::new(dir.clone());
        assert!(store.list().is_empty());
        let mut b = sample();
        b.name = "alpha".into();
        store.save(&sample()).unwrap();
        store.save(&b).unwrap();
        let names: Vec<_> = store.list().into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["alpha", "Jump Server"]);
        store.delete("alpha").unwrap();
        assert_eq!(store.list().len(), 1);
        assert!(store.delete("nope").is_err());
        assert!(store.save(&Profile { name: "///".into(), ..sample() }).is_ok()); // sanitised to "___"
        std::fs::remove_dir_all(dir).unwrap();
    }
}
