//! Where NexDesk keeps and finds things, per operating system.
use std::path::PathBuf;

/// The user's home directory (`HOME`, or `USERPROFILE` on Windows).
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// The base of per-user configuration: `$XDG_CONFIG_HOME` or `~/.config` (Linux), `~/Library/Application Support` (macOS),
/// `%APPDATA%` (Windows).
pub fn config_base() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        return std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home_dir().map(|h| h.join("AppData").join("Roaming")));
    }
    if cfg!(target_os = "macos") {
        return home_dir().map(|h| h.join("Library").join("Application Support"));
    }
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home_dir().map(|h| h.join(".config")))
}

/// The contents of `~/.config/user-dirs.dirs` (Linux desktops); `None` elsewhere or when missing.
pub fn user_dirs_file() -> Option<String> {
    if cfg!(any(target_os = "windows", target_os = "macos")) {
        return None;
    }
    std::fs::read_to_string(home_dir()?.join(".config/user-dirs.dirs")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_base_is_absolute_when_known() {
        if let Some(p) = config_base() {
            assert!(p.is_absolute());
        }
    }
}
