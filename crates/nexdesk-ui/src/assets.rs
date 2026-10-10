//! Embedded vector icons (24x24, stroke based, tinted with the element's text colour).
use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

pub struct Assets;

macro_rules! icons {
    ($($n:literal),* $(,)?) => {
        fn find(path: &str) -> Option<&'static [u8]> {
            match path {
                $( concat!("icons/", $n, ".svg") => Some(include_bytes!(concat!("../assets/icons/", $n, ".svg"))), )*
                _ => None,
            }
        }
        const NAMES: &[&str] = &[$(concat!("icons/", $n, ".svg")),*];
    };
}

icons!(
    "connections",
    "devices",
    "book",
    "sessions",
    "settings",
    "log-connection",
    "log-file",
    "log-alarm",
    "log-console",
    "logs",
    "sidebar",
    "back",
    "forward",
    "search",
    "grid",
    "list",
    "menu",
    "chevron-down",
    "chevron-right",
    "plus",
    "check",
    "close",
    "refresh",
    "trash",
    "keyboard",
    "info",
    "shield",
    "palette",
    "zap"
);

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(find(path).map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(NAMES
            .iter()
            .filter(|n| n.starts_with(path))
            .map(|n| SharedString::from(*n))
            .collect())
    }
}
