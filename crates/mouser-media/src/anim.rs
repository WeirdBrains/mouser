//! The fullscreen open/close transition: the picture flies from its
//! thumbnail rect to the window (or back) with a cubic ease-in-out while a
//! dim backdrop crossfades. Time is wall-clock so the ease stays even if a
//! frame is late.

use mouser_mode_api::ui::Rect;
use std::time::{Duration, Instant};

/// The ease and interpolation are mouser-views' (the carousel slide uses them too).
pub use mouser_views::anim::{ease, lerp_rect};

/// One open or close of a fullscreen picture or player.
#[derive(Debug, Clone, Copy)]
pub struct FsAnim {
    pub opening: bool,
    pub from: Rect,
    pub to: Rect,
    pub start: Instant,
    /// A redraw ticker is running for it.
    pub ticking: bool,
}

impl FsAnim {
    pub const DUR: Duration = Duration::from_millis(220);

    pub fn new(opening: bool, from: Rect, to: Rect) -> Self {
        FsAnim { opening, from, to, start: Instant::now(), ticking: false }
    }

    pub fn progress(&self, now: Instant) -> f64 {
        ease(now.saturating_duration_since(self.start).as_secs_f64() / Self::DUR.as_secs_f64())
    }

    pub fn finished(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.start) >= Self::DUR
    }

    pub fn rect(&self, now: Instant) -> Rect {
        lerp_rect(self.from, self.to, self.progress(now))
    }

    /// The backdrop's opacity (0 clear … 200).
    pub fn veil(&self, now: Instant) -> u8 {
        let p = if self.opening { self.progress(now) } else { 1.0 - self.progress(now) };
        (p * 200.0).round().clamp(0.0, 200.0) as u8
    }

    pub fn end(&self) -> Instant {
        self.start + Self::DUR
    }

    /// Call `wake` every 8 ms until the transition ends (once per animation).
    pub fn keep_redrawing(&mut self, wake: Option<crate::Waker>) {
        let Some(w) = wake.filter(|_| !self.ticking) else { return };
        self.ticking = true;
        let until = self.end() + Duration::from_millis(10);
        let _ = std::thread::Builder::new().name("fs-anim".into()).spawn(move || {
            while Instant::now() < until {
                std::thread::sleep(Duration::from_millis(8));
                w();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eases_slow_at_both_ends_and_flies_between_rects() {
        assert_eq!((ease(0.0), ease(1.0), ease(0.5)), (0.0, 1.0, 0.5));
        assert!(ease(0.1) < 0.1 && ease(0.9) > 0.9);
        let (from, to) = (Rect { x: 0, y: 0, w: 100, h: 50 }, Rect { x: 100, y: 100, w: 300, h: 250 });
        let a = FsAnim::new(true, from, to);
        assert_eq!(a.rect(a.start), from);
        assert_eq!(a.rect(a.start + FsAnim::DUR / 2), lerp_rect(from, to, 0.5));
        assert_eq!(a.rect(a.end()), to);
        assert!(a.finished(a.end()) && !a.finished(a.start));
        assert_eq!((a.veil(a.start), a.veil(a.end())), (0, 200));
        let c = FsAnim::new(false, to, from);
        assert_eq!((c.veil(c.start), c.veil(c.end())), (200, 0));
    }
}
