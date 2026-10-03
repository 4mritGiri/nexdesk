//! Parser for Microsoft `.rdp` connection files (`key:type:value` lines).
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Int(i64),
    /// Unknown type letter (e.g. `b` binary) or an `i` value that is not a number.
    Other {
        ty: String,
        raw: String,
    },
}

#[derive(Debug, Default, Clone)]
pub struct RdpFile {
    map: HashMap<String, Value>,
}

impl RdpFile {
    pub fn parse(text: &str) -> Self {
        let text = text.trim_start_matches('\u{feff}');
        let mut map = HashMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut it = line.splitn(3, ':');
            let (Some(k), Some(t), Some(v)) = (it.next(), it.next(), it.next()) else {
                continue; // malformed line: ignore, like mstsc does
            };
            let key = k.trim().to_ascii_lowercase();
            let value = match t.trim() {
                "s" => Value::Str(v.to_string()),
                "i" => match v.trim().parse::<i64>() {
                    Ok(n) => Value::Int(n),
                    Err(_) => Value::Other {
                        ty: "i".into(),
                        raw: v.into(),
                    },
                },
                other => Value::Other {
                    ty: other.into(),
                    raw: v.into(),
                },
            };
            map.insert(key, value);
        }
        Self { map }
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        match self.map.get(&key.to_ascii_lowercase())? {
            Value::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn get_int(&self, key: &str) -> Option<i64> {
        match self.map.get(&key.to_ascii_lowercase())? {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// `full address:s:host[:port]`
    pub fn full_address(&self) -> Option<&str> {
        self.get_str("full address")
    }
    pub fn username(&self) -> Option<&str> {
        self.get_str("username")
    }
    pub fn domain(&self) -> Option<&str> {
        self.get_str("domain")
    }
    pub fn desktop_size(&self) -> Option<(u16, u16)> {
        let w = u16::try_from(self.get_int("desktopwidth")?).ok()?;
        let h = u16::try_from(self.get_int("desktopheight")?).ok()?;
        (w > 0 && h > 0).then_some((w, h))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typical_file() {
        let f = RdpFile::parse(
            "\u{feff}full address:s:jump.corp.example:3390\r\nusername:s:CORP\\alice\r\n\
             desktopwidth:i:1920\r\ndesktopheight:i:1080\r\nscreen mode id:i:2\r\n",
        );
        assert_eq!(f.full_address(), Some("jump.corp.example:3390"));
        assert_eq!(f.username(), Some("CORP\\alice"));
        assert_eq!(f.desktop_size(), Some((1920, 1080)));
        assert_eq!(f.get_int("screen mode id"), Some(2));
    }

    #[test]
    fn keys_are_case_insensitive_and_malformed_lines_skipped() {
        let f = RdpFile::parse("Full Address:s:host\ngarbage line\nonly:s\n");
        assert_eq!(f.full_address(), Some("host"));
        assert_eq!(f.get_str("only"), None);
    }

    #[test]
    fn value_may_contain_colons() {
        let f = RdpFile::parse("full address:s:[::1]:3389");
        assert_eq!(f.full_address(), Some("[::1]:3389"));
    }

    #[test]
    fn bad_sizes_rejected() {
        let f = RdpFile::parse("desktopwidth:i:0\ndesktopheight:i:600");
        assert_eq!(f.desktop_size(), None);
        let f = RdpFile::parse("desktopwidth:i:99999\ndesktopheight:i:600");
        assert_eq!(f.desktop_size(), None);
    }
}
