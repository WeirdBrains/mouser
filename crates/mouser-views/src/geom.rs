//! Rect arithmetic for laying out and hit-testing panel items.

use mouser_mode_api::ui::Rect;

pub fn inside(r: Rect, (x, y): (i32, i32)) -> bool {
    r.w > 0 && x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h
}

pub fn overlaps(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

pub fn intersect(a: Rect, b: Rect) -> Rect {
    let (l, t) = (a.x.max(b.x), a.y.max(b.y));
    let (r, bt) = ((a.x + a.w).min(b.x + b.w), (a.y + a.h).min(b.y + b.h));
    Rect { x: l, y: t, w: (r - l).max(0), h: (bt - t).max(0) }
}

/// A pointer position packed into an action argument (`x | y << 15`).
pub fn pack_xy(x: i32, y: i32) -> i32 {
    x.clamp(0, 0x7fff) | (y.clamp(0, 0x7fff) << 15)
}

pub fn unpack_xy(a: i32) -> (i32, i32) {
    (a & 0x7fff, (a >> 15) & 0x7fff)
}

/// An image of `size` pixels drawn at `dest`, cut to the visible `clip`:
/// the part of the image to show (x, y, w, h in image pixels) and where it
/// goes, or None when nothing of it shows.
pub fn crop(dest: Rect, size: (u32, u32), clip: Rect) -> Option<((u32, u32, u32, u32), Rect)> {
    let (l, t) = (dest.x.max(clip.x), dest.y.max(clip.y));
    let (r, b) = ((dest.x + dest.w).min(clip.x + clip.w), (dest.y + dest.h).min(clip.y + clip.h));
    if r <= l || b <= t || dest.w <= 0 || dest.h <= 0 {
        return None;
    }
    let (sx, sy) = (size.0 as f64 / dest.w as f64, size.1 as f64 / dest.h as f64);
    let px = |v: i32, s: f64, max: u32| ((v as f64 * s) as u32).min(max);
    let (x0, y0) = (px(l - dest.x, sx, size.0), px(t - dest.y, sy, size.1));
    let (x1, y1) = (px(r - dest.x, sx, size.0), px(b - dest.y, sy, size.1));
    Some(((x0, y0, (x1 - x0).max(1), (y1 - y0).max(1)), Rect { x: l, y: t, w: r - l, h: b - t }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rects_pack_and_crop() {
        let a = Rect { x: 0, y: 0, w: 10, h: 10 };
        let b = Rect { x: 5, y: 5, w: 10, h: 10 };
        assert!(overlaps(a, b) && inside(a, (9, 9)) && !inside(a, (10, 0)));
        assert_eq!(intersect(a, b), Rect { x: 5, y: 5, w: 5, h: 5 });
        assert_eq!(unpack_xy(pack_xy(1234, 567)), (1234, 567));
        // The right half of a 100x50 image drawn 200 wide.
        let (src, at) = crop(Rect { x: 0, y: 0, w: 200, h: 100 }, (100, 50), Rect { x: 100, y: 0, w: 500, h: 500 }).unwrap();
        assert_eq!((src, at), ((50, 0, 50, 50), Rect { x: 100, y: 0, w: 100, h: 100 }));
        assert!(crop(a, (1, 1), Rect { x: 20, y: 20, w: 5, h: 5 }).is_none());
    }
}
