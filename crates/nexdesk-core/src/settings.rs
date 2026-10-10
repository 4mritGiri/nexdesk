//! Application preferences, stored as `key=value` lines in `~/.config/nexdesk/settings`.
//! Unknown keys are kept out; bad values fall back to defaults, so a hand-edited file never breaks start-up.
use std::io;
use std::path::PathBuf;

use crate::profiles::Speed;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    /// Deep navy (default).
    #[default]
    Midnight,
    /// Neutral dark grey, like GNOME Files.
    Graphite,
    /// Light, for bright rooms.
    Light,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::Midnight, Theme::Graphite, Theme::Light];

    pub fn label(self) -> &'static str {
        match self {
            Theme::Midnight => "Midnight",
            Theme::Graphite => "Graphite",
            Theme::Light => "Light",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Theme::Midnight => "midnight",
            Theme::Graphite => "graphite",
            Theme::Light => "light",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.key() == s)
    }
}

/// How the session engine treats unknown / changed server certificates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TlsMode {
    /// Ask in the session window (default, safest for everyday use).
    #[default]
    Ask,
    /// Trust the first certificate silently, refuse changes.
    AcceptNew,
    /// Refuse anything not already pinned or trusted by the system.
    Strict,
}

impl TlsMode {
    pub const ALL: [TlsMode; 3] = [TlsMode::Ask, TlsMode::AcceptNew, TlsMode::Strict];
    pub fn label(self) -> &'static str {
        match self {
            TlsMode::Ask => "Ask",
            TlsMode::AcceptNew => "Trust first",
            TlsMode::Strict => "Strict",
        }
    }
    pub fn cli(self) -> &'static str {
        match self {
            TlsMode::Ask => "ask",
            TlsMode::AcceptNew => "accept-new",
            TlsMode::Strict => "strict",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.cli() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub theme: Theme,
    /// Show connections as tiles (true) or list (false) at start.
    pub grid_view: bool,
    pub sidebar_collapsed: bool,
    /// Defaults used when creating a new connection.
    pub default_speed: Speed,
    pub start_fullscreen: bool,
    pub tls: TlsMode,
    /// Send Super / Alt+Tab to the remote computer while full screen.
    pub key_capture: bool,
    /// Use the system title bar for the session window instead of NexDesk's own header.
    pub native_frame: bool,
    /// Run the session window through X11/XWayland (needed for drag & drop of files on Wayland).
    pub prefer_x11: bool,
    /// After dropping files on a session, press Ctrl+V on the remote automatically.
    pub drop_paste: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::Midnight,
            grid_view: true,
            sidebar_collapsed: false,
            default_speed: Speed::Balanced,
            start_fullscreen: false,
            tls: TlsMode::Ask,
            key_capture: true,
            native_frame: false,
            prefer_x11: false,
            drop_paste: true,
        }
    }
}

pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("nexdesk").join("settings"))
}

impl Settings {
    pub fn parse(text: &str) -> Self {
        let mut s = Settings::default();
        let b = |v: &str, d: bool| match v {
            "true" | "1" | "yes" => true,
            "false" | "0" | "no" => false,
            _ => d,
        };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "theme" => s.theme = Theme::parse(v).unwrap_or(s.theme),
                "grid_view" => s.grid_view = b(v, s.grid_view),
                "sidebar_collapsed" => s.sidebar_collapsed = b(v, s.sidebar_collapsed),
                "default_speed" => s.default_speed = Speed::parse(v).unwrap_or(s.default_speed),
                "start_fullscreen" => s.start_fullscreen = b(v, s.start_fullscreen),
                "tls" => s.tls = TlsMode::parse(v).unwrap_or(s.tls),
                "key_capture" => s.key_capture = b(v, s.key_capture),
                "native_frame" => s.native_frame = b(v, s.native_frame),
                "prefer_x11" => s.prefer_x11 = b(v, s.prefer_x11),
                "drop_paste" => s.drop_paste = b(v, s.drop_paste),
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        format!(
            "theme={}\ngrid_view={}\nsidebar_collapsed={}\ndefault_speed={}\nstart_fullscreen={}\ntls={}\nkey_capture={}\nnative_frame={}\nprefer_x11={}\ndrop_paste={}\n",
            self.theme.key(),
            self.grid_view,
            self.sidebar_collapsed,
            self.default_speed.cli(),
            self.start_fullscreen,
            self.tls.cli(),
            self.key_capture,
            self.native_frame,
            self.prefer_x11,
            self.drop_paste
        )
    }

    pub fn load() -> Self {
        default_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| Self::parse(&t))
            .unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        let Some(path) = default_path() else {
            return Ok(());
        };
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let s = Settings {
            theme: Theme::Graphite,
            grid_view: false,
            default_speed: Speed::Slow,
            tls: TlsMode::Strict,
            key_capture: false,
            prefer_x11: true,
            ..Settings::default()
        };
        assert_eq!(Settings::parse(&s.to_text()), s);
    }

    #[test]
    fn bad_input_falls_back() {
        let s =
            Settings::parse("theme=neon\ntls=whatever\ngrid_view=maybe\njunk\n=\nkey_capture=no\n");
        assert_eq!(s.theme, Theme::Midnight);
        assert_eq!(s.tls, TlsMode::Ask);
        assert!(s.grid_view);
        assert!(!s.key_capture);
    }
}
