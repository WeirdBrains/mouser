//! Easing in wall-clock time, so a late frame doesn't slow the motion.

use mouser_mode_api::ui::Rect;
use std::time::{Duration, Instant};

/// Ease-in-out (cubic): slow start, fast middle, slow stop.
pub fn ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
}

fn lerp_i(a: i32, b: i32, t: f64) -> i32 {
    (a as f64 + (b - a) as f64 * t).round() as i32
}

/// The rect `t` (0..1) of the way from `a` to `b`.
pub fn lerp_rect(a: Rect, b: Rect, t: f64) -> Rect {
    Rect { x: lerp_i(a.x, b.x, t), y: lerp_i(a.y, b.y, t), w: lerp_i(a.w, b.w, t), h: lerp_i(a.h, b.h, t) }
}

/// Carousel slide: eases the shown position from where it was to the
/// selection over [`Slide::DUR`]; a new target mid-slide starts from the
/// current position, and a far jump (Home/End, search) starts one item
/// away instead of flying past everything.
#[derive(Debug, Clone, Copy, Default)]
pub struct Slide {
    from: f64,
    to: f64,
    start: Option<Instant>,
    /// A redraw ticker is running for this slide (the mode sets it).
    pub ticking: bool,
}

impl Slide {
    pub const DUR: Duration = Duration::from_millis(180);

    /// The same ease as [`ease`].
    pub fn ease(t: f64) -> f64 {
        ease(t)
    }

    fn value(&self, now: Instant) -> f64 {
        match self.start {
            Some(s) => self.from + (self.to - self.from) * ease(now.saturating_duration_since(s).as_secs_f64() / Self::DUR.as_secs_f64()),
            None => self.to,
        }
    }

    /// The position to draw for `target` at `now` (retargets smoothly).
    pub fn at(&mut self, target: f64, now: Instant) -> f64 {
        if self.start.is_none() && self.from == 0.0 && self.to == 0.0 {
            self.to = target;
            self.from = target;
        }
        if target != self.to {
            let cur = self.value(now);
            self.from = if (target - cur).abs() > 3.0 { target - (target - cur).signum() * 1.0 } else { cur };
            self.to = target;
            self.start = Some(now);
            self.ticking = false;
        }
        let v = self.value(now);
        if self.start.is_some_and(|s| now.saturating_duration_since(s) >= Self::DUR) {
            self.start = None;
            self.from = self.to;
            self.ticking = false;
            return self.to;
        }
        v
    }

    pub fn moving(&self) -> bool {
        self.start.is_some()
    }

    /// When the current slide ends (now when it isn't moving).
    pub fn end(&self) -> Instant {
        self.start.map_or_else(Instant::now, |s| s + Self::DUR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_is_slow_at_both_ends_and_half_way_at_the_middle() {
        assert_eq!((ease(0.0), ease(0.5), ease(1.0)), (0.0, 0.5, 1.0));
        assert!(ease(0.1) < 0.1 && ease(0.9) > 0.9);
        assert_eq!(ease(-1.0), 0.0);
        let (a, b) = (Rect { x: 0, y: 0, w: 10, h: 10 }, Rect { x: 100, y: 50, w: 30, h: 20 });
        assert_eq!(lerp_rect(a, b, 0.5), Rect { x: 50, y: 25, w: 20, h: 15 });
    }

    #[test]
    fn slides_ease_to_the_target_and_far_jumps_start_one_away() {
        let t0 = Instant::now();
        let mut s = Slide::default();
        assert_eq!(s.at(2.0, t0), 2.0, "the first position is where it starts");
        assert!(!s.moving());
        let mid = s.at(3.0, t0 + Duration::from_millis(1));
        assert!(s.moving() && (2.0..3.0).contains(&mid));
        assert_eq!(s.at(3.0, t0 + Slide::DUR * 2), 3.0);
        assert!(!s.moving());
        let far = s.at(40.0, t0 + Slide::DUR * 3);
        assert!((39.0..40.0).contains(&far), "{far}");
    }
}
