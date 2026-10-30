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

/// Coverage bitmap (`w` x `h`, row-major) for `c`, or `None` if it is not handled.
pub fn render(c: char, w: i32, h: i32) -> Option<Vec<u8>> {
    let [up, right, down, left] = arms(c)?;
    let (w, h) = (w.max(1), h.max(1));
    let light = (h / 16).max(1);
    let thick = |k: u8| if k == 2 { light * 2 + 1 } else { light };
    let mut bmp = vec![0u8; (w * h) as usize];
    let mut fill = |x0: i32, y0: i32, x1: i32, y1: i32| {
        for y in y0.max(0)..y1.min(h) {
            for x in x0.max(0)..x1.min(w) {
                bmp[(y * w + x) as usize] = 255;
            }
        }
    };
    let (cx, cy) = (w / 2, h / 2);
    // Horizontal arms meet vertical ones over the widest vertical stroke, and vice versa.
    let vt = thick(up.max(down));
    let ht = thick(left.max(right));
    let span = |t: i32, c: i32| (c - t / 2, c - t / 2 + t);
    if left > 0 {
        let (y0, y1) = span(thick(left), cy);
        fill(0, y0, cx - vt / 2 + vt, y1);
    }
    if right > 0 {
        let (y0, y1) = span(thick(right), cy);
        fill(cx - vt / 2, y0, w, y1);
    }
    if up > 0 {
        let (x0, x1) = span(thick(up), cx);
        fill(x0, 0, x1, cy - ht / 2 + ht);
    }
    if down > 0 {
        let (x0, x1) = span(thick(down), cx);
        fill(x0, cy - ht / 2, x1, h);
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

    #[test]
    fn table_covers_cross_and_ignores_letters() {
        assert_eq!(arms('┼'), Some([1, 1, 1, 1]));
        assert_eq!(arms('╋'), Some([2, 2, 2, 2]));
        assert_eq!(arms('┳'), Some([0, 2, 2, 2]));
        assert_eq!(arms('A'), None);
        assert_eq!(arms('═'), None);
    }
}
