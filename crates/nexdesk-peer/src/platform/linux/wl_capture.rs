//! Wayland capture: the picture comes from the desktop portal's PipeWire stream.
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::frame::{dirty_rects, extract};
use crate::wire::{pack_pixels, Msg};
use crate::PeerError;

fn cap(e: impl std::fmt::Display) -> PeerError {
    PeerError::Capture(e.to_string())
}

/// A Wayland screen: the picture comes from the portal's PipeWire stream.
pub struct WlCapture {
    wl: Arc<super::wayland::Wl>,
    pub(super) w: u16,
    pub(super) h: u16,
    prev: Vec<u8>,
    last_seq: u64,
    last_grab: Instant,
}

impl WlCapture {
    pub(super) fn new(wl: Arc<super::wayland::Wl>) -> Result<Self, PeerError> {
        let (w, h) = wl
            .wait_first_frame(Duration::from_secs(15))
            .ok_or_else(|| cap("the desktop did not deliver any picture"))?;
        if w == 0 || h == 0 || w > u32::from(u16::MAX) || h > u32::from(u16::MAX) {
            return Err(cap(format!("unsupported screen size {w}x{h}")));
        }
        Ok(Self {
            wl,
            w: w as u16,
            h: h as u16,
            prev: Vec::new(),
            last_seq: 0,
            last_grab: Instant::now(),
        })
    }

    pub(super) fn layout_changed(&self) -> bool {
        matches!(self.wl.state(), Some((w, h, _)) if w != u32::from(self.w) || h != u32::from(self.h))
    }

    pub(super) fn next_update(&mut self) -> Result<Vec<Msg>, PeerError> {
        let Some((w, h, seq)) = self.wl.state() else {
            return Ok(Vec::new());
        };
        // an idle screen costs nothing; a full check at least once a second as a safety net
        if seq == self.last_seq
            && !self.prev.is_empty()
            && self.last_grab.elapsed() < Duration::from_secs(1)
        {
            return Ok(Vec::new());
        }
        if w != u32::from(self.w) || h != u32::from(self.h) {
            return Err(cap("the screen size changed"));
        }
        let Some((_, _, cur)) = self.wl.grab() else {
            return Ok(Vec::new());
        };
        self.last_seq = seq;
        self.last_grab = Instant::now();
        let (w, h) = (self.w as usize, self.h as usize);
        let rects = dirty_rects(&self.prev, &cur, w, h);
        let msgs = rects
            .into_iter()
            .map(|(x, y, rw, rh)| Msg::Tile {
                x: x as u16,
                y: y as u16,
                w: rw as u16,
                h: rh as u16,
                lz4: pack_pixels(&extract(&cur, w, (x, y, rw, rh))),
            })
            .collect();
        self.prev = cur;
        Ok(msgs)
    }
}

