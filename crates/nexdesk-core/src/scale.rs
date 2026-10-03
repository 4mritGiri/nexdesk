//! Aspect-preserving fit of the remote framebuffer into a window, plus the
//! inverse mapping for mouse coordinates. Pixels are `0x00RRGGBB` `u32`s.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub src_w: u32,
    pub src_h: u32,
    pub dst_w: u32,
    pub dst_h: u32,
    pub scale: f64,
    pub off_x: u32,
    pub off_y: u32,
    pub draw_w: u32,
    pub draw_h: u32,
}

impl Fit {
    pub fn new(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Option<Self> {
        if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
            return None;
        }
        let scale = (dst_w as f64 / src_w as f64).min(dst_h as f64 / src_h as f64);
        let draw_w = ((src_w as f64 * scale).round() as u32).clamp(1, dst_w);
        let draw_h = ((src_h as f64 * scale).round() as u32).clamp(1, dst_h);
        Some(Self {
            src_w,
            src_h,
            dst_w,
            dst_h,
            scale,
            off_x: (dst_w - draw_w) / 2,
            off_y: (dst_h - draw_h) / 2,
            draw_w,
            draw_h,
        })
    }

    /// Window (physical pixel) position -> remote desktop position, clamped to the desktop.
    pub fn to_remote(&self, x: f64, y: f64) -> (u16, u16) {
        let rx = ((x - self.off_x as f64) / self.scale).floor();
        let ry = ((y - self.off_y as f64) / self.scale).floor();
        let rx = rx.clamp(0.0, (self.src_w - 1) as f64) as u32;
        let ry = ry.clamp(0.0, (self.src_h - 1) as f64) as u32;
        (rx as u16, ry as u16)
    }
}

/// Nearest-neighbour blit of `src` (sw x sh) into `dst` (dw x dh), letterboxed.
pub fn blit_fit(src: &[u32], sw: u32, sh: u32, dst: &mut [u32], dw: u32, dh: u32) {
    if src.len() < (sw as usize) * (sh as usize) || dst.len() < (dw as usize) * (dh as usize) {
        return;
    }
    let Some(fit) = Fit::new(sw, sh, dw, dh) else { return };
    dst[..(dw as usize) * (dh as usize)].fill(0);

    let xmap: Vec<u32> = (0..fit.draw_w)
        .map(|x| (((x as f64) / fit.scale) as u32).min(sw - 1))
        .collect();

    for y in 0..fit.draw_h {
        let sy = (((y as f64) / fit.scale) as u32).min(sh - 1);
        let srow = &src[(sy * sw) as usize..((sy + 1) * sw) as usize];
        let drow_start = ((y + fit.off_y) * dw + fit.off_x) as usize;
        let drow = &mut dst[drow_start..drow_start + fit.draw_w as usize];
        if fit.draw_w == sw {
            drow.copy_from_slice(srow);
        } else {
            for (d, &sx) in drow.iter_mut().zip(&xmap) {
                *d = srow[sx as usize];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_fit() {
        let f = Fit::new(100, 50, 100, 50).unwrap();
        assert_eq!((f.off_x, f.off_y, f.draw_w, f.draw_h), (0, 0, 100, 50));
        assert_eq!(f.to_remote(10.5, 20.2), (10, 20));
    }

    #[test]
    fn letterbox_and_mouse_mapping() {
        // 100x50 into 200x200 => scale 2, drawn 200x100, centred vertically (offset 50)
        let f = Fit::new(100, 50, 200, 200).unwrap();
        assert_eq!((f.off_x, f.off_y, f.draw_w, f.draw_h), (0, 50, 200, 100));
        assert_eq!(f.to_remote(100.0, 100.0), (50, 25));
        // inside the black bar clamps to the edge
        assert_eq!(f.to_remote(100.0, 0.0), (50, 0));
        assert_eq!(f.to_remote(5000.0, 5000.0), (99, 49));
    }

    #[test]
    fn zero_sizes_are_none() {
        assert!(Fit::new(0, 1, 1, 1).is_none());
        assert!(Fit::new(1, 1, 0, 1).is_none());
    }

    #[test]
    fn blit_upscales_and_leaves_bars_black() {
        let src = [1u32, 2, 3, 4]; // 2x2
        let mut dst = vec![9u32; 4 * 6]; // 4 wide, 6 tall => scale 2, draw 4x4, bars of 1 row
        blit_fit(&src, 2, 2, &mut dst, 4, 6);
        assert_eq!(&dst[0..4], &[0, 0, 0, 0]);
        assert_eq!(&dst[4..8], &[1, 1, 2, 2]);
        assert_eq!(&dst[8..12], &[1, 1, 2, 2]);
        assert_eq!(&dst[12..16], &[3, 3, 4, 4]);
        assert_eq!(&dst[20..24], &[0, 0, 0, 0]);
    }

    #[test]
    fn blit_same_size_copies() {
        let src: Vec<u32> = (0..12).collect();
        let mut dst = vec![0u32; 12];
        blit_fit(&src, 4, 3, &mut dst, 4, 3);
        assert_eq!(src, dst);
    }
}
