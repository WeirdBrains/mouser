//! Carousel geometry: items on a horizontal strip, the one at `pos` (a
//! fractional index while sliding, see [`crate::anim::Slide`]) centred and
//! its neighbours the same size, cut off by the edges.

use mouser_mode_api::ui::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Strip {
    pub body: Rect,
    /// Width of each item.
    pub w: i32,
    pub gap: i32,
}

impl Strip {
    /// Items `w` wide `gap` apart in `body`.
    pub fn new(body: Rect, w: i32, gap: i32) -> Self {
        Strip { body, w, gap }
    }

    /// Item `i`'s full rect (height `h` from the top of the body) at `pos`.
    pub fn rect(&self, i: usize, pos: f64, h: i32) -> Rect {
        let x_mid = self.body.x + (self.body.w - self.w) / 2;
        Rect { x: x_mid + ((i as f64 - pos) * (self.w + self.gap) as f64).round() as i32, y: self.body.y, w: self.w, h }
    }

    /// The part of `full` on the strip, or None when less than `min` px show.
    pub fn visible(&self, full: Rect, min: i32) -> Option<Rect> {
        let (l, r) = (full.x.max(self.body.x), (full.x + full.w).min(self.body.x + self.body.w));
        (r - l >= min).then_some(Rect { x: l, w: r - l, ..full })
    }

    /// Items to draw for `len` items at `pos` with `selected` the target:
    /// those on the strip and the selection's neighbours (the incoming one
    /// while sliding), the nearest the middle first so it loads first.
    pub fn order(pos: f64, selected: usize, len: usize) -> Vec<usize> {
        if len == 0 {
            return vec![];
        }
        let sel = selected as i64;
        let lo = (pos.floor() as i64 - 1).min(sel - 1).max(0) as usize;
        let hi = ((pos.ceil() as i64 + 1).max(sel + 1).max(0) as usize).min(len - 1);
        let mut v: Vec<usize> = (lo..=hi).collect();
        v.sort_by(|a, b| (*a as f64 - pos).abs().total_cmp(&(*b as f64 - pos).abs()));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_selection_is_centred_and_neighbours_are_cut_off() {
        let s = Strip::new(Rect { x: 0, y: 10, w: 1000, h: 400 }, 550, 20);
        let mid = s.rect(3, 3.0, 300);
        assert_eq!(mid, Rect { x: 225, y: 10, w: 550, h: 300 });
        let right = s.rect(4, 3.0, 300);
        assert_eq!(right.x, 225 + 570);
        assert_eq!(s.visible(right, 8).unwrap().w, 1000 - 795);
        assert!(s.visible(s.rect(6, 3.0, 300), 8).is_none());
        assert_eq!(Strip::order(3.0, 3, 10), [3, 2, 4]);
        assert_eq!(Strip::order(3.5, 4, 10)[..2], [3, 4]);
        assert_eq!(Strip::order(0.0, 0, 1), [0]);
        assert!(Strip::order(0.0, 0, 0).is_empty());
    }
}
