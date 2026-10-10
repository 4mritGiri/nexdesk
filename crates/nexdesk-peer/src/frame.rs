//! Picture differencing shared by every capture backend: which 64-pixel tiles changed between two BGRX frames.

pub const TILE: usize = 64;

/// Changed regions between two BGRX screens: per 64-pixel tile row, runs of adjacent changed tiles.
/// With an empty or differently sized `prev` everything is changed.
pub fn dirty_rects(
    prev: &[u8],
    cur: &[u8],
    w: usize,
    h: usize,
) -> Vec<(usize, usize, usize, usize)> {
    let full = prev.len() != cur.len();
    let mut out = Vec::new();
    let stride = w * 4;
    for ty in (0..h).step_by(TILE) {
        let th = TILE.min(h - ty);
        let mut run: Option<usize> = None;
        let flush = |start: Option<usize>, end_x: usize, out: &mut Vec<_>| {
            if let Some(sx) = start {
                out.push((sx, ty, end_x - sx, th));
            }
        };
        for tx in (0..w).step_by(TILE) {
            let tw = TILE.min(w - tx);
            let changed = full
                || (0..th).any(|r| {
                    let o = (ty + r) * stride + tx * 4;
                    prev[o..o + tw * 4] != cur[o..o + tw * 4]
                });
            match (changed, run) {
                (true, None) => run = Some(tx),
                (false, Some(_)) => {
                    flush(run.take(), tx, &mut out);
                }
                _ => {}
            }
        }
        flush(run, w, &mut out);
    }
    out
}

pub fn extract(cur: &[u8], w: usize, (x, y, rw, rh): (usize, usize, usize, usize)) -> Vec<u8> {
    let mut out = Vec::with_capacity(rw * rh * 4);
    for r in 0..rh {
        let o = ((y + r) * w + x) * 4;
        out.extend_from_slice(&cur[o..o + rw * 4]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_rects_find_only_what_changed() {
        let (w, h) = (200usize, 130usize);
        let a = vec![0u8; w * h * 4];
        // first frame (no previous): everything, merged into one run per tile row
        let all = dirty_rects(&[], &a, w, h);
        assert_eq!(all.len(), 3, "rows: 64 + 64 + 2 px");
        assert!(all.iter().all(|r| r.0 == 0 && r.2 == w));
        assert_eq!(all[2], (0, 128, w, 2));
        // identical frames: nothing
        assert!(dirty_rects(&a, &a, w, h).is_empty());
        // change one pixel at (70, 10): tile column 1, tile row 0
        let mut b = a.clone();
        b[(10 * w + 70) * 4] = 9;
        assert_eq!(dirty_rects(&a, &b, w, h), vec![(64, 0, 64, 64)]);
        // change pixels in tile columns 0 and 2 of row 1: two separate runs
        let mut c = a.clone();
        c[(70 * w + 3) * 4] = 1;
        c[(70 * w + 140) * 4] = 1;
        assert_eq!(
            dirty_rects(&a, &c, w, h),
            vec![(0, 64, 64, 64), (128, 64, 64, 64)]
        );
        // last partial column / row
        let mut d = a.clone();
        d[(129 * w + 199) * 4 + 1] = 5;
        assert_eq!(dirty_rects(&a, &d, w, h), vec![(192, 128, 8, 2)]);
    }

    #[test]
    fn extract_copies_the_right_pixels() {
        let w = 4;
        let cur: Vec<u8> = (0..4 * 3 * 4).map(|i| i as u8).collect();
        let e = extract(&cur, w, (1, 1, 2, 2));
        assert_eq!(e.len(), 16);
        assert_eq!(&e[..8], &cur[(w + 1) * 4..(w + 1) * 4 + 8]);
    }
}
