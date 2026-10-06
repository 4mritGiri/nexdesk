//! Screenshot of the remote desktop: saved as PNG into the user's Pictures folder.
//! Only the local framebuffer is written; nothing is sent anywhere.
use std::path::{Path, PathBuf};

/// `XDG_PICTURES_DIR="$HOME/Pictures"` from `~/.config/user-dirs.dirs`, else `~/Pictures`.
pub fn parse_pictures_dir(user_dirs: Option<&str>, home: &Path) -> PathBuf {
    if let Some(text) = user_dirs {
        for line in text.lines() {
            let line = line.trim();
            if let Some(v) = line.strip_prefix("XDG_PICTURES_DIR=") {
                let v = v.trim().trim_matches('"');
                if let Some(rest) = v.strip_prefix("$HOME") {
                    let rest = rest.trim_start_matches('/');
                    return if rest.is_empty() { home.to_path_buf() } else { home.join(rest) };
                }
                if v.starts_with('/') && !v.contains("..") {
                    return PathBuf::from(v);
                }
            }
        }
    }
    home.join("Pictures")
}

/// `nexdesk-<host>-YYYYMMDD-HHMMSS.png`, with the host reduced to safe characters.
pub fn file_name(host: &str, stamp: &str) -> String {
    let host: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .take(40)
        .collect();
    let host = host.trim_matches(|c| c == '.' || c == '_').to_owned();
    let host = if host.is_empty() { "remote".to_owned() } else { host };
    let digits: Vec<char> = stamp.chars().filter(|c| c.is_ascii_digit()).collect();
    let (d, t) = if digits.len() >= 14 {
        (digits[..8].iter().collect::<String>(), digits[8..14].iter().collect::<String>())
    } else {
        ("00000000".into(), "000000".into())
    };
    format!("nexdesk-{host}-{d}-{t}.png")
}

/// 0x00RRGGBB pixels -> packed RGB bytes.
pub fn rgb_bytes(buf: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(buf.len() * 3);
    for p in buf {
        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    out
}

fn unique(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let stem = name.trim_end_matches(".png");
    (1..1000)
        .map(|i| dir.join(format!("{stem}-{i}.png")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// Save into `dir` (created if needed). Returns the written path.
pub fn save_in(dir: &Path, buf: &[u32], w: u32, h: u32, host: &str, stamp: &str) -> Result<PathBuf, String> {
    if w == 0 || h == 0 || buf.len() < (w as usize) * (h as usize) {
        return Err("no picture to save yet".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = unique(dir, &file_name(host, stamp));
    let img = image::RgbImage::from_raw(w, h, rgb_bytes(&buf[..(w as usize) * (h as usize)]))
        .ok_or("bad image size")?;
    img.save_with_format(&path, image::ImageFormat::Png)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

/// Save into the user's Pictures folder using the current local time.
pub fn save(buf: &[u32], w: u32, h: u32, host: &str) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or("HOME is not set")?;
    let dirs = std::fs::read_to_string(home.join(".config/user-dirs.dirs")).ok();
    let dir = parse_pictures_dir(dirs.as_deref(), &home);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    save_in(&dir, buf, w, h, host, &nexdesk_core::logs::format_time(now_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_dir_parsing() {
        let home = Path::new("/home/u");
        assert_eq!(parse_pictures_dir(None, home), Path::new("/home/u/Pictures"));
        let t = "# c\nXDG_DOWNLOAD_DIR=\"$HOME/Dl\"\nXDG_PICTURES_DIR=\"$HOME/Bilder\"\n";
        assert_eq!(parse_pictures_dir(Some(t), home), Path::new("/home/u/Bilder"));
        assert_eq!(parse_pictures_dir(Some("XDG_PICTURES_DIR=\"/data/pics\""), home), Path::new("/data/pics"));
        // relative / traversal values are ignored
        assert_eq!(parse_pictures_dir(Some("XDG_PICTURES_DIR=\"/a/../etc\""), home), Path::new("/home/u/Pictures"));
        assert_eq!(parse_pictures_dir(Some("XDG_PICTURES_DIR=\"rel\""), home), Path::new("/home/u/Pictures"));
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("srv-01.corp", "2026-10-06 16:05:09"), "nexdesk-srv-01.corp-20261006-160509.png");
        assert_eq!(file_name("../../etc/passwd", "2026-10-06 16:05:09"), "nexdesk-etc_passwd-20261006-160509.png");
        assert!(!file_name("a/b\\c", "x").contains('/'));
        assert_eq!(file_name("", "2026-10-06 16:05:09"), "nexdesk-remote-20261006-160509.png");
    }

    #[test]
    fn writes_a_png_and_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("nexdesk-shot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let buf = vec![0x00_11_22_33u32; 4 * 3];
        let a = save_in(&dir, &buf, 4, 3, "h", "2026-01-02 03:04:05").unwrap();
        let b = save_in(&dir, &buf, 4, 3, "h", "2026-01-02 03:04:05").unwrap();
        assert_ne!(a, b);
        let img = image::open(&a).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (4, 3));
        assert_eq!(img.get_pixel(0, 0).0, [0x11, 0x22, 0x33]);
        assert!(save_in(&dir, &buf, 0, 3, "h", "").is_err());
        assert!(save_in(&dir, &buf[..5], 4, 3, "h", "").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
