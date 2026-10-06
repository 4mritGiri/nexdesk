//! Saved connections, stored as standard `.rdp` files (one per connection),
//! so they are also readable by mstsc/Remmina/FreeRDP. Passwords are NEVER stored.
use std::io;
use std::path::{Path, PathBuf};

use crate::rdpfile::RdpFile;

/// Connection speed preset (mstsc's "connection type"). Controls how much visual
/// decoration the server renders and sends: less data = faster on slow links.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Speed {
    /// Fast local network: everything on (wallpaper, animations, 32-bit colour).
    Lan,
    /// Default: no wallpaper, menu animations or full-window drag; 32-bit colour.
    #[default]
    Balanced,
    /// Slow / mobile / VPN-over-internet: also no themes or cursor effects, 16-bit colour.
    Slow,
}

impl Speed {
    pub const ALL: [Speed; 3] = [Speed::Lan, Speed::Balanced, Speed::Slow];

    pub fn label(self) -> &'static str {
        match self {
            Speed::Lan => "LAN",
            Speed::Balanced => "Balanced",
            Speed::Slow => "Slow network",
        }
    }

    pub fn cli(self) -> &'static str {
        match self {
            Speed::Lan => "lan",
            Speed::Balanced => "balanced",
            Speed::Slow => "slow",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "lan" => Some(Speed::Lan),
            "balanced" | "auto" => Some(Speed::Balanced),
            "slow" | "wan" => Some(Speed::Slow),
            _ => None,
        }
    }

    /// mstsc `connection type:i:` value.
    fn rdp_value(self) -> u8 {
        match self {
            Speed::Lan => 6,
            Speed::Balanced => 7,
            Speed::Slow => 2,
        }
    }

    fn from_rdp_value(v: Option<i64>) -> Self {
        match v {
            Some(6) => Speed::Lan,
            Some(1) | Some(2) | Some(3) => Speed::Slow,
            _ => Speed::Balanced,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Speed::Lan => Speed::Balanced,
            Speed::Balanced => Speed::Slow,
            Speed::Slow => Speed::Lan,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub host: String,
    pub user: String,
    pub domain: String,
    pub width: u16,
    pub height: u16,
    pub clipboard: bool,
    /// Start the session full screen (mstsc: `screen mode id:i:2`).
    pub fullscreen: bool,
    pub speed: Speed,
    /// Program (plus arguments, no shell) run before connecting; a failure aborts the connection.
    pub pre_command: String,
    /// Program run after the session ends.
    pub post_command: String,
}

impl Profile {
    /// Stable, filesystem-safe identity used by the session and credential layers.
    pub fn profile_id(&self) -> String {
        file_stem_for(&self.name)
            .unwrap_or_else(|| "connection".into())
            .to_ascii_lowercase()
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
            fullscreen: false,
            speed: Speed::Balanced,
            pre_command: String::new(),
            post_command: String::new(),
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
            fullscreen: f.get_int("screen mode id") == Some(2),
            speed: Speed::from_rdp_value(f.get_int("connection type")),
            // Never taken from a file that may come from somebody else; see `from_rdp_trusted`.
            pre_command: String::new(),
            post_command: String::new(),
        }
    }

    /// Like `from_rdp`, but also reads the NexDesk command hooks. Only for NexDesk's own store.
    pub fn from_rdp_trusted(name: &str, f: &RdpFile) -> Self {
        let mut p = Self::from_rdp(name, f);
        p.pre_command = f.get_str("nexdesk pre command").unwrap_or("").to_string();
        p.post_command = f.get_str("nexdesk post command").unwrap_or("").to_string();
        p
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
        s.push_str(&format!("connection type:i:{}\r\n", self.speed.rdp_value()));
        s.push_str(&format!(
            "screen mode id:i:{}\r\n",
            if self.fullscreen { 2 } else { 1 }
        ));
        if !self.pre_command.trim().is_empty() {
            s.push_str(&format!("nexdesk pre command:s:{}\r\n", clean(self.pre_command.trim())));
        }
        if !self.post_command.trim().is_empty() {
            s.push_str(&format!("nexdesk post command:s:{}\r\n", clean(self.post_command.trim())));
        }
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
        crate::hooks::validate(&self.pre_command).map_err(|_| "Before-connect command is not valid (check quotes)")?;
        crate::hooks::validate(&self.post_command).map_err(|_| "After-disconnect command is not valid (check quotes)")?;
        Ok(())
    }
}

/// Make a user-typed name safe as a file stem.
pub fn file_stem_for(name: &str) -> Option<String> {
    let s: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
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
        let stem = file_stem_for(&p.name).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid connection name")
        })?;
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(self.dir.join(format!("{stem}.rdp")), p.to_rdp_text())
    }

    pub fn delete(&self, name: &str) -> io::Result<()> {
        let stem = file_stem_for(name).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid connection name")
        })?;
        std::fs::remove_file(self.dir.join(format!("{stem}.rdp")))
    }

    /// All saved profiles, sorted by name (case-insensitive). Unreadable files are skipped.
    pub fn list(&self) -> Vec<Profile> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return out;
        };
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("rdp") {
                continue;
            }
            let (Some(stem), Ok(text)) = (
                path.file_stem().and_then(|s| s.to_str()),
                std::fs::read_to_string(&path),
            ) else {
                continue;
            };
            out.push(Profile::from_rdp_trusted(stem, &RdpFile::parse(&text)));
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
            fullscreen: true,
            speed: Speed::Slow,
            pre_command: "nmcli con up \"Work VPN\"".into(),
            post_command: "nmcli con down id Work".into(),
        }
    }

    #[test]
    fn imported_files_cannot_smuggle_commands() {
        let p = sample();
        let text = p.to_rdp_text();
        assert!(text.contains("nexdesk pre command:s:"));
        let imported = Profile::from_rdp("x", &RdpFile::parse(&text));
        assert!(imported.pre_command.is_empty() && imported.post_command.is_empty());
        let bad = Profile { pre_command: "echo 'open".into(), ..sample() };
        assert!(bad.validate().is_err());
        // newlines cannot add extra keys
        let nl = Profile { pre_command: "a\r\nfull address:s:evil".into(), ..sample() };
        assert_eq!(Profile::from_rdp_trusted("n", &RdpFile::parse(&nl.to_rdp_text())).host, p.host);
    }

    #[test]
    fn roundtrip_through_rdp_text() {
        let p = sample();
        let back = Profile::from_rdp_trusted("Jump Server", &RdpFile::parse(&p.to_rdp_text()));
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
        assert_eq!(
            file_stem_for("../../etc/passwd").as_deref(),
            Some("_.._etc_passwd")
        );
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
        assert!(store
            .save(&Profile {
                name: "///".into(),
                ..sample()
            })
            .is_ok()); // sanitised to "___"
        std::fs::remove_dir_all(dir).unwrap();
    }
}
