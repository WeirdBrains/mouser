//! Procedural box-drawing glyphs (U+2500-U+257F light and heavy lines).
//!
//! Font glyphs for these rarely fill the whole cell, so adjacent cells show
//! gaps and lines look dashed. Drawing them from the cell size makes every
//! arm reach the cell edge exactly.

/// Arm weights (up, right, down, left): 0 none, 1 light, 2 heavy.
pub fn arms(c: char) -> Option<[u8; 4]> {
    let v = c as u32;
    let a = |s: &[u8; 4]| Some(*s);
    match v {
        0x2500 => a(&[0, 1, 0, 1]),
        0x2501 => a(&[0, 2, 0, 2]),
        0x2502 => a(&[1, 0, 1, 0]),
        0x2503 => a(&[2, 0, 2, 0]),
        0x250C..=0x254B => {
            // (U, R, D, L) for U+250C onwards, in code point order.
            const T: [[u8; 4]; 64] = [
                [0, 1, 1, 0],
                [0, 2, 1, 0],
                [0, 1, 2, 0],
                [0, 2, 2, 0],
                [0, 0, 1, 1],
                [0, 0, 1, 2],
                [0, 0, 2, 1],
                [0, 0, 2, 2],
                [1, 1, 0, 0],
                [1, 2, 0, 0],
                [2, 1, 0, 0],
                [2, 2, 0, 0],
                [1, 0, 0, 1],
                [1, 0, 0, 2],
                [2, 0, 0, 1],
                [2, 0, 0, 2],
                [1, 1, 1, 0],
                [1, 2, 1, 0],
                [2, 1, 1, 0],
                [1, 1, 2, 0],
                [2, 1, 2, 0],
                [2, 2, 1, 0],
                [1, 2, 2, 0],
                [2, 2, 2, 0],
                [1, 0, 1, 1],
                [1, 0, 1, 2],
                [2, 0, 1, 1],
                [1, 0, 2, 1],
                [2, 0, 2, 1],
                [2, 0, 1, 2],
                [1, 0, 2, 2],
                [2, 0, 2, 2],
                [0, 1, 1, 1],
                [0, 1, 1, 2],
                [0, 2, 1, 1],
                [0, 2, 1, 2],
                [0, 1, 2, 1],
                [0, 1, 2, 2],
                [0, 2, 2, 1],
                [0, 2, 2, 2],
                [1, 1, 0, 1],
                [1, 1, 0, 2],
                [1, 2, 0, 1],
                [1, 2, 0, 2],
                [2, 1, 0, 1],
                [2, 1, 0, 2],
                [2, 2, 0, 1],
                [2, 2, 0, 2],
                [1, 1, 1, 1],
                [1, 1, 1, 2],
                [1, 2, 1, 1],
                [1, 2, 1, 2],
                [2, 1, 1, 1],
                [1, 1, 2, 1],
                [2, 1, 2, 1],
                [2, 1, 1, 2],
                [2, 2, 1, 1],
                [1, 1, 2, 2],
                [1, 2, 2, 1],
                [2, 2, 1, 2],
                [1, 2, 2, 2],
                [2, 1, 2, 2],
                [2, 2, 2, 1],
                [2, 2, 2, 2],
            ];
            Some(T[(v - 0x250C) as usize])
        }
        // Rounded corners drawn square.
        0x256D => a(&[0, 1, 1, 0]),
        0x256E => a(&[0, 0, 1, 1]),
        0x256F => a(&[1, 0, 0, 1]),
        0x2570 => a(&[1, 1, 0, 0]),
        0x2574..=0x257F => {
            const T: [[u8; 4]; 12] = [
                [0, 0, 0, 1],
                [1, 0, 0, 0],
                [0, 1, 0, 0],
                [0, 0, 1, 0],
                [0, 0, 0, 2],
                [2, 0, 0, 0],
                [0, 2, 0, 0],
                [0, 0, 2, 0],
                [0, 2, 0, 1],
                [1, 0, 2, 0],
                [0, 1, 0, 2],
                [2, 0, 1, 0],
            ];
            Some(T[(v - 0x2574) as usize])
        }
        _ => None,
    }
}

/// Integer-pixel stroke geometry shared by every box-drawing glyph, so lines
/// in neighbouring cells (and in the CPU and GPU renderers, which both blit
/// these bitmaps at integer cell origins) always meet at the same offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    /// Light stroke width in pixels.
    pub light: i32,
    /// Heavy stroke width (spans exactly the same pixels as a double line).
    pub heavy: i32,
}

impl Geometry {
    pub fn new(w: i32, h: i32) -> Self {
        let light = (w.min(h * 2) / 9).max(1).min((h / 16).max(1)).max(1);
        Self { light, heavy: light * 3 }
    }
    /// Offset of a stroke of width `t` centred in a cell of size `cell`.
    pub fn start(cell: i32, t: i32) -> i32 {
        (cell - t).div_euclid(2)
    }
    /// The two strokes of a double line: offsets of the outer and inner line.
    pub fn double(&self, cell: i32) -> (i32, i32) {
        let a = Self::start(cell, self.heavy);
        (a, a + 2 * self.light)
    }
}

/// Double-line characters this module draws itself.
fn is_double(c: char) -> bool {
    matches!(c, '═' | '║' | '╔' | '╗' | '╚' | '╝' | '╠' | '╣' | '╦' | '╩' | '╬')
}

/// Coverage bitmap (`w` x `h`, row-major) for `c`, or `None` if it is not handled.
pub fn render(c: char, w: i32, h: i32) -> Option<Vec<u8>> {
    let (w, h) = (w.max(1), h.max(1));
    let g = Geometry::new(w, h);
    let mut bmp = vec![0u8; (w * h) as usize];
    let mut fill = |x0: i32, y0: i32, x1: i32, y1: i32| {
        for y in y0.max(0)..y1.min(h) {
            for x in x0.max(0)..x1.min(w) {
                bmp[(y * w + x) as usize] = 255;
            }
        }
    };
    if is_double(c) {
        let t = g.light;
        let (ax, bx) = g.double(w);
        let (ay, by) = g.double(h);
        let hl = |f: &mut dyn FnMut(i32, i32, i32, i32), y: i32, x0: i32, x1: i32| f(x0, y, x1, y + t);
        let vl = |f: &mut dyn FnMut(i32, i32, i32, i32), x: i32, y0: i32, y1: i32| f(x, y0, x + t, y1);
        let f = &mut fill;
        match c {
            '═' => {
                hl(f, ay, 0, w);
                hl(f, by, 0, w);
            }
            '║' => {
                vl(f, ax, 0, h);
                vl(f, bx, 0, h);
            }
            '╔' => {
                hl(f, ay, ax, w);
                vl(f, ax, ay, h);
                hl(f, by, bx, w);
                vl(f, bx, by, h);
            }
            '╗' => {
                hl(f, ay, 0, bx + t);
                vl(f, bx, ay, h);
                hl(f, by, 0, ax + t);
                vl(f, ax, by, h);
            }
            '╚' => {
                vl(f, ax, 0, by + t);
                hl(f, by, ax, w);
                vl(f, bx, 0, ay + t);
                hl(f, ay, bx, w);
            }
            '╝' => {
                vl(f, bx, 0, by + t);
                hl(f, by, 0, bx + t);
                vl(f, ax, 0, ay + t);
                hl(f, ay, 0, ax + t);
            }
            '╠' => {
                vl(f, ax, 0, h);
                vl(f, bx, 0, ay + t);
                vl(f, bx, by, h);
                hl(f, ay, bx, w);
                hl(f, by, bx, w);
            }
            '╣' => {
                vl(f, bx, 0, h);
                vl(f, ax, 0, ay + t);
                vl(f, ax, by, h);
                hl(f, ay, 0, ax + t);
                hl(f, by, 0, ax + t);
            }
            '╦' => {
                hl(f, ay, 0, w);
                hl(f, by, 0, ax + t);
                hl(f, by, bx, w);
                vl(f, ax, by, h);
                vl(f, bx, by, h);
            }
            '╩' => {
                hl(f, by, 0, w);
                hl(f, ay, 0, ax + t);
                hl(f, ay, bx, w);
                vl(f, ax, 0, ay + t);
                vl(f, bx, 0, ay + t);
            }
            _ => {
                // ╬
                for y in [ay, by] {
                    hl(f, y, 0, ax + t);
                    hl(f, y, bx, w);
                }
                for x in [ax, bx] {
                    vl(f, x, 0, ay + t);
                    vl(f, x, by, h);
                }
            }
        }
        return Some(bmp);
    }
    let [up, right, down, left] = arms(c)?;
    let thick = |k: u8| if k == 2 { g.heavy } else { g.light };
    // Horizontal arms reach across the widest vertical stroke, and vice versa.
    let vt = thick(up.max(down));
    let ht = thick(left.max(right));
    let (vx, hy) = (Geometry::start(w, vt), Geometry::start(h, ht));
    let ys = |k: u8| (Geometry::start(h, thick(k)), Geometry::start(h, thick(k)) + thick(k));
    let xs = |k: u8| (Geometry::start(w, thick(k)), Geometry::start(w, thick(k)) + thick(k));
    if left > 0 {
        let (y0, y1) = ys(left);
        fill(0, y0, vx + vt, y1);
    }
    if right > 0 {
        let (y0, y1) = ys(right);
        fill(vx, y0, w, y1);
    }
    if up > 0 {
        let (x0, x1) = xs(up);
        fill(x0, 0, x1, hy + ht);
    }
    if down > 0 {
        let (x0, x1) = xs(down);
        fill(x0, hy, x1, h);
    }
    Some(bmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(b: &[u8], w: i32, x: i32, y: i32) -> u8 {
        b[(y * w + x) as usize]
    }

    #[test]
    fn horizontal_line_spans_full_width() {
        let (w, h) = (9, 19);
        let b = render('─', w, h).unwrap();
        assert!((0..w).all(|x| at(&b, w, x, h / 2) == 255));
        assert_eq!(at(&b, w, 0, 0), 0);
    }

    #[test]
    fn vertical_line_spans_full_height() {
        let (w, h) = (9, 19);
        let b = render('│', w, h).unwrap();
        assert!((0..h).all(|y| at(&b, w, w / 2, y) == 255));
    }

    #[test]
    fn corner_reaches_edges_and_joins() {
        let (w, h) = (9, 19);
        let b = render('┌', w, h).unwrap();
        assert_eq!(at(&b, w, w - 1, h / 2), 255);
        assert_eq!(at(&b, w, w / 2, h - 1), 255);
        assert_eq!(at(&b, w, w / 2, h / 2), 255);
        assert_eq!(at(&b, w, 0, h / 2), 0);
        assert_eq!(at(&b, w, w / 2, 0), 0);
    }

    /// Rows (or columns) inked along a scan line through the cell.
    fn inked(b: &[u8], w: i32, h: i32, vertical: bool, at_: i32) -> Vec<i32> {
        if vertical { (0..h).filter(|&y| at(b, w, at_, y) != 0).collect() } else { (0..w).filter(|&x| at(b, w, x, at_) != 0).collect() }
    }

    #[test]
    fn single_heavy_and_double_share_one_centre() {
        for (w, h) in [(7, 15), (8, 16), (9, 19), (10, 21), (11, 24), (14, 30), (17, 37)] {
            let g = Geometry::new(w, h);
            // Horizontal strokes, sampled down column 0.
            let light = inked(&render('─', w, h).unwrap(), w, h, true, 0);
            let heavy = inked(&render('━', w, h).unwrap(), w, h, true, 0);
            let dbl = inked(&render('═', w, h).unwrap(), w, h, true, 0);
            assert_eq!(light.len() as i32, g.light, "{w}x{h}");
            assert_eq!(heavy.first(), dbl.first(), "{w}x{h}");
            assert_eq!(heavy.last(), dbl.last(), "{w}x{h}");
            // The light line sits exactly between the two double lines.
            assert_eq!(light[0], dbl[0] + 2 * g.light - g.light, "{w}x{h}");
            // Every light glyph with a horizontal arm uses the same rows.
            for c in ['┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '┼', '╶', '╴'] {
                let b = render(c, w, h).unwrap();
                let edge = if inked(&b, w, h, true, w - 1).is_empty() { 0 } else { w - 1 };
                assert_eq!(inked(&b, w, h, true, edge), light, "{c} {w}x{h}");
            }
            // And vertical arms the same columns.
            let col = inked(&render('│', w, h).unwrap(), w, h, false, 0);
            for c in ['┌', '┐', '├', '┤', '┬', '┼'] {
                let b = render(c, w, h).unwrap();
                assert_eq!(inked(&b, w, h, false, h - 1), col, "{c} {w}x{h}");
            }
            let dcol = inked(&render('║', w, h).unwrap(), w, h, false, 0);
            for c in ['╔', '╗', '╦', '╬', '╠', '╣'] {
                let b = render(c, w, h).unwrap();
                assert_eq!(inked(&b, w, h, false, h - 1), dcol, "{c} {w}x{h}");
            }
        }
    }

    #[test]
    fn table_covers_cross_and_ignores_letters() {
        assert_eq!(arms('┼'), Some([1, 1, 1, 1]));
        assert_eq!(arms('╋'), Some([2, 2, 2, 2]));
        assert_eq!(arms('┳'), Some([0, 2, 2, 2]));
        assert_eq!(arms('A'), None);
        assert_eq!(arms('═'), None);
        assert!(render('═', 9, 19).is_some());
    }
}
