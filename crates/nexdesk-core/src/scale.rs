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
    let Some(fit) = Fit::new(sw, sh, dw, dh) else {
        return;
    };
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

// ---------------------------------------------------------------- actual size (1:1) with panning

/// 1:1 view of the remote desktop. When the desktop is larger than the window only a part is
/// visible (`pan_*` = the remote pixel shown at the window's top-left); when it is smaller it is
/// centred. Pan values are clamped, so callers may keep stale values after a resize.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Actual {
    pub src_w: u32,
    pub src_h: u32,
    pub dst_w: u32,
    pub dst_h: u32,
    pub pan_x: u32,
    pub pan_y: u32,
}

impl Actual {
    pub fn new(
        src_w: u32,
        src_h: u32,
        dst_w: u32,
        dst_h: u32,
        pan_x: u32,
        pan_y: u32,
    ) -> Option<Self> {
        if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
            return None;
        }
        let mut a = Self {
            src_w,
            src_h,
            dst_w,
            dst_h,
            pan_x,
            pan_y,
        };
        let (mx, my) = a.max_pan();
        a.pan_x = pan_x.min(mx);
        a.pan_y = pan_y.min(my);
        Some(a)
    }

    /// Largest useful pan in each direction (0 when the desktop fits).
    pub fn max_pan(&self) -> (u32, u32) {
        (
            self.src_w.saturating_sub(self.dst_w),
            self.src_h.saturating_sub(self.dst_h),
        )
    }

    pub fn pannable(&self) -> bool {
        let (mx, my) = self.max_pan();
        mx > 0 || my > 0
    }

    /// Where remote pixel (0, 0) lands in the window (negative when panned).
    pub fn origin(&self) -> (i64, i64) {
        let ox = if self.src_w <= self.dst_w {
            i64::from(self.dst_w - self.src_w) / 2
        } else {
            -i64::from(self.pan_x)
        };
        let oy = if self.src_h <= self.dst_h {
            i64::from(self.dst_h - self.src_h) / 2
        } else {
            -i64::from(self.pan_y)
        };
        (ox, oy)
    }

    pub fn to_remote(&self, x: f64, y: f64) -> (u16, u16) {
        let (ox, oy) = self.origin();
        let rx = (x - ox as f64).floor().clamp(0.0, (self.src_w - 1) as f64) as u32;
        let ry = (y - oy as f64).floor().clamp(0.0, (self.src_h - 1) as f64) as u32;
        (
            rx.min(u32::from(u16::MAX)) as u16,
            ry.min(u32::from(u16::MAX)) as u16,
        )
    }
}

/// Copy the visible part of `src` into `dst` (1:1), black around it.
pub fn blit_actual(
    src: &[u32],
    sw: u32,
    sh: u32,
    dst: &mut [u32],
    dw: u32,
    dh: u32,
    pan_x: u32,
    pan_y: u32,
) {
    if src.len() < (sw as usize) * (sh as usize) || dst.len() < (dw as usize) * (dh as usize) {
        return;
    }
    let Some(a) = Actual::new(sw, sh, dw, dh, pan_x, pan_y) else {
        return;
    };
    dst[..(dw as usize) * (dh as usize)].fill(0);
    let (ox, oy) = a.origin();
    // window rows/cols that show remote pixels
    let x0 = ox.max(0) as u32;
    let x1 = ((ox + i64::from(sw)).min(i64::from(dw))).max(0) as u32;
    let y0 = oy.max(0) as u32;
    let y1 = ((oy + i64::from(sh)).min(i64::from(dh))).max(0) as u32;
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    for y in y0..y1 {
        let sy = (i64::from(y) - oy) as usize;
        let sx0 = (i64::from(x0) - ox) as usize;
        let n = (x1 - x0) as usize;
        let s = sy * sw as usize + sx0;
        let d = y as usize * dw as usize + x0 as usize;
        dst[d..d + n].copy_from_slice(&src[s..s + n]);
    }
}

/// The two ways the remote desktop can be shown in the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum View {
    Fit(Fit),
    Actual(Actual),
}

impl View {
    pub fn to_remote(&self, x: f64, y: f64) -> (u16, u16) {
        match self {
            View::Fit(f) => f.to_remote(x, y),
            View::Actual(a) => a.to_remote(x, y),
        }
    }

    pub fn blit(&self, src: &[u32], dst: &mut [u32]) {
        match self {
            View::Fit(f) => blit_fit(src, f.src_w, f.src_h, dst, f.dst_w, f.dst_h),
            View::Actual(a) => blit_actual(
                src, a.src_w, a.src_h, dst, a.dst_w, a.dst_h, a.pan_x, a.pan_y,
            ),
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

    #[test]
    fn actual_centres_small_desktops_and_pans_large_ones() {
        // desktop smaller than window: centred, no panning
        let a = Actual::new(100, 50, 300, 200, 40, 40).unwrap();
        assert!(!a.pannable());
        assert_eq!(a.origin(), (100, 75));
        assert_eq!(a.to_remote(100.0, 75.0), (0, 0));
        assert_eq!(a.to_remote(0.0, 0.0), (0, 0)); // outside clamps
        assert_eq!(a.to_remote(999.0, 999.0), (99, 49));
        // desktop larger: pan is clamped and shifts the mapping
        let a = Actual::new(1000, 800, 400, 300, 5000, 100).unwrap();
        assert_eq!(a.max_pan(), (600, 500));
        assert_eq!((a.pan_x, a.pan_y), (600, 100));
        assert!(a.pannable());
        assert_eq!(a.to_remote(0.0, 0.0), (600, 100));
        assert_eq!(a.to_remote(399.0, 299.0), (999, 399));
        assert!(Actual::new(0, 1, 1, 1, 0, 0).is_none());
    }

    #[test]
    fn blit_actual_shows_the_panned_window() {
        // 4x3 source, 2x2 window panned by (1,1) shows source pixels (1..3, 1..3)
        let src: Vec<u32> = (1..=12).collect(); // rows: 1 2 3 4 / 5 6 7 8 / 9 10 11 12
        let mut dst = vec![99u32; 4];
        blit_actual(&src, 4, 3, &mut dst, 2, 2, 1, 1);
        assert_eq!(dst, vec![6, 7, 10, 11]);
        // smaller than the window: centred with black borders
        let src = [7u32, 8, 9, 10]; // 2x2
        let mut dst = vec![5u32; 4 * 4];
        blit_actual(&src, 2, 2, &mut dst, 4, 4, 0, 0);
        assert_eq!(&dst[0..4], &[0, 0, 0, 0]);
        assert_eq!(&dst[4..8], &[0, 7, 8, 0]);
        assert_eq!(&dst[8..12], &[0, 9, 10, 0]);
        assert_eq!(&dst[12..16], &[0, 0, 0, 0]);
    }

    #[test]
    fn view_dispatches_to_the_active_mode() {
        let fit = View::Fit(Fit::new(100, 50, 200, 100).unwrap());
        assert_eq!(fit.to_remote(100.0, 50.0), (50, 25));
        let act = View::Actual(Actual::new(100, 50, 200, 100, 0, 0).unwrap());
        assert_eq!(act.to_remote(50.0, 25.0), (0, 0)); // centred: origin (50,25)
        let src = vec![1u32; 100 * 50];
        let mut dst = vec![0u32; 200 * 100];
        act.blit(&src, &mut dst);
        assert_eq!(dst[25 * 200 + 50], 1);
        assert_eq!(dst[0], 0);
    }
}
