//! Mouse wheel and trackpad scrolling.
//!
//! A wheel notch arrives as `LineDelta` (Mouser scrolls 3 lines per notch).
//! A trackpad (Wayland/macOS) sends many small `PixelDelta`s, often just a
//! few pixels each; truncating each one to whole lines threw them all away,
//! so pixels are accumulated here and only whole lines come out, with the
//! remainder kept for the next event.

use winit::event::MouseScrollDelta;

/// Lines scrolled per wheel notch.
pub const LINES_PER_NOTCH: f64 = 3.0;

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct WheelAccum {
    /// Fractional lines not yet scrolled (positive = up / back in history).
    rest: f64,
}

/// One scroll step: `lines` to scroll (positive = up) and `reports`, the wheel
/// button events to send to a program that asked for mouse reports.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WheelStep {
    pub lines: isize,
    pub reports: isize,
}

impl WheelAccum {
    pub fn feed(&mut self, delta: MouseScrollDelta, cell_h: f32) -> WheelStep {
        let (add, pixels) = match delta {
            MouseScrollDelta::LineDelta(_, y) => (y as f64 * LINES_PER_NOTCH, false),
            MouseScrollDelta::PixelDelta(p) => (p.y / (cell_h.max(1.0) as f64), true),
        };
        // Changing direction drops what was left over the other way.
        if add != 0.0 && self.rest != 0.0 && add.signum() != self.rest.signum() {
            self.rest = 0.0;
        }
        self.rest += add;
        let lines = self.rest.trunc();
        self.rest -= lines;
        let lines = lines as isize;
        // A notch is one wheel report; a trackpad sends one per line, like a
        // fine-grained wheel, so it scrolls an app at the same speed as the grid.
        let reports = if pixels { lines } else { lines.signum() * lines.unsigned_abs().div_ceil(LINES_PER_NOTCH as usize) as isize };
        WheelStep { lines, reports }
    }

    pub fn reset(&mut self) {
        self.rest = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::dpi::PhysicalPosition;

    fn px(y: f64) -> MouseScrollDelta {
        MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, y))
    }

    #[test]
    fn small_pixel_deltas_add_up_to_lines() {
        let mut a = WheelAccum::default();
        // 20px cells, 3px per event: nothing for six events, then one line.
        let got: Vec<isize> = (0..14).map(|_| a.feed(px(3.0), 20.0).lines).collect();
        assert_eq!(got.iter().sum::<isize>(), 2, "{got:?}");
        assert_eq!(got[..6], [0; 6]);
        assert_eq!(got[6], 1);
    }

    #[test]
    fn pixel_lines_are_reported_one_wheel_event_each() {
        let mut a = WheelAccum::default();
        assert_eq!(a.feed(px(-45.0), 10.0), WheelStep { lines: -4, reports: -4 });
        assert_eq!(a.feed(px(-5.0), 10.0), WheelStep { lines: -1, reports: -1 });
    }

    #[test]
    fn direction_change_drops_the_remainder() {
        let mut a = WheelAccum::default();
        assert_eq!(a.feed(px(15.0), 20.0).lines, 0);
        assert_eq!(a.feed(px(-15.0), 20.0).lines, 0);
        assert_eq!(a.feed(px(-5.0), 20.0).lines, -1);
    }

    #[test]
    fn wheel_notches_are_three_lines_one_report() {
        let mut a = WheelAccum::default();
        assert_eq!(a.feed(MouseScrollDelta::LineDelta(0.0, 1.0), 20.0), WheelStep { lines: 3, reports: 1 });
        assert_eq!(a.feed(MouseScrollDelta::LineDelta(0.0, -2.0), 20.0), WheelStep { lines: -6, reports: -2 });
        // Fractional (high-resolution) notches accumulate too.
        assert_eq!(a.feed(MouseScrollDelta::LineDelta(0.0, 0.2), 20.0).lines, 0);
        assert_eq!(a.feed(MouseScrollDelta::LineDelta(0.0, 0.2), 20.0), WheelStep { lines: 1, reports: 1 });
    }
}
