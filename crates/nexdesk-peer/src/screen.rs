//! Viewer-side picture of the remote screen, assembled from tiles.
use crate::wire::{unpack_pixels, Msg};
use crate::PeerError;

pub struct Screen {
    pub w: u32,
    pub h: u32,
    /// 0x00RRGGBB, ready for softbuffer.
    pub buf: Vec<u32>,
}

impl Screen {
    pub fn new(w: u16, h: u16) -> Self {
        Self { w: w as u32, h: h as u32, buf: vec![0; w as usize * h as usize] }
    }

    /// Paint a `Tile` message; tiles outside the screen are an error (never written out of bounds).
    pub fn apply(&mut self, m: &Msg) -> Result<(), PeerError> {
        let Msg::Tile { x, y, w, h, lz4 } = m else { return Ok(()) };
        let (x, y, w, h) = (*x as u32, *y as u32, *w as u32, *h as u32);
        if x + w > self.w || y + h > self.h {
            return Err(PeerError::Proto("tile outside the screen"));
        }
        let px = unpack_pixels(lz4, w as u16, h as u16)?;
        for row in 0..h {
            let dst = ((y + row) * self.w + x) as usize;
            let src = (row * w * 4) as usize;
            for col in 0..w as usize {
                let p = &px[src + col * 4..src + col * 4 + 4];
                self.buf[dst + col] = u32::from(p[2]) << 16 | u32::from(p[1]) << 8 | u32::from(p[0]);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::pack_pixels;

    #[test]
    fn tiles_land_where_they_belong_and_bounds_are_checked() {
        let mut s = Screen::new(8, 4);
        // 2x2 tile of BGRX = (b=1,g=2,r=3)
        let px: Vec<u8> = std::iter::repeat([1u8, 2, 3, 0]).take(4).flatten().collect();
        let m = Msg::Tile { x: 6, y: 2, w: 2, h: 2, lz4: pack_pixels(&px) };
        s.apply(&m).unwrap();
        assert_eq!(s.buf[2 * 8 + 6], 0x00_03_02_01);
        assert_eq!(s.buf[3 * 8 + 7], 0x00_03_02_01);
        assert_eq!(s.buf[0], 0);
        let off = Msg::Tile { x: 7, y: 0, w: 2, h: 2, lz4: pack_pixels(&px) };
        assert!(s.apply(&off).is_err());
    }
}
