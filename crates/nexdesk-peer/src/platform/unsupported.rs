//! Platforms without a screen-sharing backend yet (Windows, macOS). The viewer works there; sharing the
//! screen of such a computer reports a clear error. Add a backend here by providing the same API as `linux`.
use crate::wire::Msg;
use crate::PeerError;

const NOT_YET: &str = "sharing this computer's screen is not supported on this operating system yet (the viewer works; the agent runs on Linux)";

pub struct Display;

impl Display {
    pub fn open(_view_only: bool) -> Result<Self, PeerError> {
        Err(PeerError::Capture(NOT_YET.into()))
    }

    pub fn capture(&self, _selected: usize) -> Result<Capture, PeerError> {
        Err(PeerError::Capture(NOT_YET.into()))
    }

    pub fn injector(&self) -> Option<Injector> {
        None
    }
}

/// Never constructed: `Display::open` fails first. Kept so the session code is identical on every platform.
pub enum Capture {}

impl Capture {
    pub fn monitor_count(&self) -> usize {
        match *self {}
    }
    pub fn selected(&self) -> usize {
        match *self {}
    }
    pub fn region(&self) -> (i16, i16, u16, u16) {
        match *self {}
    }
    pub fn cursor(&mut self) -> Option<Msg> {
        match *self {}
    }
    pub fn layout_changed(&self) -> bool {
        match *self {}
    }
    pub fn announce(&self, _view_only: bool) -> [Msg; 2] {
        match *self {}
    }
    pub fn next_update(&mut self) -> Result<Vec<Msg>, PeerError> {
        match *self {}
    }
}

pub enum Injector {}

impl Injector {
    pub fn set_region(&self, _x: i16, _y: i16, _w: u16, _h: u16) {
        match *self {}
    }
    pub fn move_to(&self, _x: u16, _y: u16) -> Result<(), PeerError> {
        match *self {}
    }
    pub fn button(&self, _button: u8, _down: bool) -> Result<(), PeerError> {
        match *self {}
    }
    pub fn wheel(&self, _dx: i16, _dy: i16) -> Result<(), PeerError> {
        match *self {}
    }
    pub fn key(&self, _code: u16, _down: bool) -> Result<(), PeerError> {
        match *self {}
    }
}
