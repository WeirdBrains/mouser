//! Image zoom: fit to the width (a tall picture pans), contain (fullscreen),
//! actual pixels, and zooming about the pointer.

use mouser_mode_api::ui::Rect;
use std::path::PathBuf;

/// Zoom of one picture: scale (1.0 = actual pixels) and the image point
/// (0..1) at the centre of the body.
#[derive(Clone, Debug, PartialEq)]
pub struct Zoom {
    /// The picture it belongs to.
    pub path: PathBuf,
    pub scale: f64,
    pub cx: f64,
    pub cy: f64,
    /// Fit (follows the window); false once zoomed.
    pub fit: bool,
}

/// Scale that fits a `nat`-sized image to the WIDTH of `body`.
pub fn fit_scale(body: Rect, nat: (u32, u32)) -> f64 {
    if nat.0 == 0 || nat.1 == 0 {
        return 1.0;
    }
    body.w as f64 / nat.0 as f64
}

/// Scale that shows the whole image inside `body`.
pub fn contain_scale(body: Rect, nat: (u32, u32)) -> f64 {
    if nat.0 == 0 || nat.1 == 0 {
        return 1.0;
    }
    (body.w as f64 / nat.0 as f64).min(body.h as f64 / nat.1 as f64)
}

/// Where a zoomed image goes: centred when smaller than the body, else
/// placed by the centre point (clamped so no gap shows at the edges).
pub fn zoom_rect(body: Rect, nat: (u32, u32), z: &mut Zoom) -> Rect {
    let (dw, dh) = (nat.0 as f64 * z.scale, nat.1 as f64 * z.scale);
    let axis = |b0: i32, bl: i32, d: f64, c: &mut f64| -> i32 {
        if d <= bl as f64 {
            *c = 0.5;
            b0 + ((bl as f64 - d) / 2.0) as i32
        } else {
            let half = bl as f64 / 2.0 / d;
            *c = c.clamp(half, 1.0 - half);
            (b0 as f64 + bl as f64 / 2.0 - *c * d).round() as i32
        }
    };
    let x = axis(body.x, body.w, dw, &mut z.cx);
    let y = axis(body.y, body.h, dh, &mut z.cy);
    Rect { x, y, w: dw.round().max(1.0) as i32, h: dh.round().max(1.0) as i32 }
}

/// Zoom by `factor` keeping the image point under `anchor` still (half the
/// fit scale … 16×).
pub fn zoom_about(body: Rect, nat: (u32, u32), z: &mut Zoom, factor: f64, anchor: (i32, i32)) {
    let r = zoom_rect(body, nat, z);
    let u = ((anchor.0 - r.x) as f64 / r.w as f64).clamp(0.0, 1.0);
    let v = ((anchor.1 - r.y) as f64 / r.h as f64).clamp(0.0, 1.0);
    let fit = fit_scale(body, nat);
    let s1 = (z.scale * factor).clamp(fit.min(1.0) * 0.5, 16.0);
    let (dw, dh) = (nat.0 as f64 * s1, nat.1 as f64 * s1);
    z.cx = u + (body.x as f64 + body.w as f64 / 2.0 - anchor.0 as f64) / dw;
    z.cy = v + (body.y as f64 + body.h as f64 / 2.0 - anchor.1 as f64) / dh;
    z.scale = s1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_contain_and_zoom_about_the_pointer() {
        let body = Rect { x: 0, y: 0, w: 400, h: 300 };
        let nat = (800, 1600);
        assert_eq!(fit_scale(body, nat), 0.5);
        assert_eq!(contain_scale(body, nat), 300.0 / 1600.0);
        let mut z = Zoom { path: "a.png".into(), scale: 0.5, cx: 0.5, cy: 0.0, fit: true };
        let r = zoom_rect(body, nat, &mut z);
        assert_eq!((r.x, r.y, r.w, r.h), (0, 0, 400, 800), "top of a tall picture, full width");
        let before = zoom_rect(body, nat, &mut z.clone());
        let p = (100, 100);
        let (u, v) = ((p.0 - before.x) as f64 / before.w as f64, (p.1 - before.y) as f64 / before.h as f64);
        zoom_about(body, nat, &mut z, 2.0, p);
        let after = zoom_rect(body, nat, &mut z);
        let back = (after.x as f64 + u * after.w as f64, after.y as f64 + v * after.h as f64);
        assert!((back.0 - 100.0).abs() <= 1.0 && (back.1 - 100.0).abs() <= 1.0, "the point under the pointer stays: {back:?}");
        assert_eq!(z.scale, 1.0);
    }
}
