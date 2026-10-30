//! SGR paint: backgrounds, underline styles, strike, cursor shapes, modified keys.
use mouser_render::frame::paint;
use mouser_render::text::{Canvas, FontStack};
use mouser_render::{CellPaint, CursorShape, Frame, PADDING, render_frame};
use winit::keyboard::ModifiersState;

const BG: u32 = 0x04192A;
const FG: u32 = 0xE6FBFA;

fn render(text: &str, styles: Vec<CellPaint>, cursor: Option<(usize, usize)>, shape: CursorShape) -> (Vec<u32>, usize, FontStack) {
    let mut fonts = FontStack::embedded(16.0);
    let w = (PADDING * 2 + fonts.cell_w * text.chars().count() as i32) as usize;
    let h = (PADDING * 2 + fonts.cell_h) as usize;
    let mut buf = vec![0u32; w * h];
    let lines = vec![text.to_string()];
    let rows = vec![styles];
    let mut canvas = Canvas::new(&mut buf, w, h);
    let frame = Frame {
        lines: &lines,
        cursor,
        bg: BG,
        fg: FG,
        ui: None,
        status: None,
        accent: None,
        cursor_color: Some(0xFF0000),
        images: &[],
        styles: &rows,
        cursor_shape: shape,
        tabs: None,
    };
    render_frame(&mut canvas, &mut fonts, &frame);
    (buf, w, fonts)
}

fn column_has(buf: &[u32], w: usize, x: usize, color: u32) -> Vec<usize> {
    (0..buf.len() / w).filter(|y| buf[y * w + x] == color).collect()
}

#[test]
fn background_fills_the_cell() {
    let mut p = CellPaint::plain(FG);
    p.bg = Some(0x00FF00);
    let (buf, w, f) = render(" ", vec![p], None, CursorShape::Block);
    let y = PADDING as usize + 1;
    assert_eq!(buf[y * w + PADDING as usize + 1], 0x00FF00);
    assert_eq!(column_has(&buf, w, PADDING as usize + 1, 0x00FF00).len(), f.cell_h as usize);
}

#[test]
fn underline_styles_draw_distinct_shapes() {
    let mk = |u: u8| CellPaint { underline: u, ul_color: 0x0000FF, ..CellPaint::plain(FG) };
    let rows = |u: u8| {
        let (buf, w, f) = render(" ", vec![mk(u)], None, CursorShape::Block);
        (0..f.cell_w as usize).map(|dx| column_has(&buf, w, PADDING as usize + dx, 0x0000FF).len()).collect::<Vec<_>>()
    };
    let single = rows(1);
    assert!(single.iter().all(|&n| n == 1));
    assert!(rows(2).iter().all(|&n| n == 2));
    let dotted = rows(4);
    assert!(dotted.contains(&0) && dotted.contains(&1));
    // Curly varies in height across the cell.
    let (buf, w, f) = render(" ", vec![mk(3)], None, CursorShape::Block);
    let ys: std::collections::BTreeSet<usize> = (0..f.cell_w as usize).flat_map(|dx| column_has(&buf, w, PADDING as usize + dx, 0x0000FF)).collect();
    assert!(ys.len() >= 2);
}

#[test]
fn strike_and_hidden() {
    let p = CellPaint { flags: paint::STRIKE | paint::NO_GLYPH, fg: 0xFF00FF, ..CellPaint::plain(FG) };
    let (buf, w, f) = render("X", vec![p], None, CursorShape::Block);
    let hits = column_has(&buf, w, PADDING as usize + 1, 0xFF00FF);
    assert_eq!(hits, vec![(PADDING + f.cell_h / 2) as usize]);
}

#[test]
fn cursor_shapes() {
    let count = |shape| {
        let (buf, _, _) = render(" ", vec![CellPaint::plain(FG)], Some((0, 0)), shape);
        buf.iter().filter(|&&p| p == 0xFF0000).count()
    };
    let f = FontStack::embedded(16.0);
    assert_eq!(count(CursorShape::Block), (f.cell_w * f.cell_h) as usize);
    assert_eq!(count(CursorShape::Bar), (2 * f.cell_h) as usize);
    assert_eq!(count(CursorShape::Underline), (2 * f.cell_w) as usize);
}

#[test]
fn modified_cursor_and_function_keys() {
    let ctrl = ModifiersState::CONTROL;
    assert_eq!(mouser_render::modified_csi(b"\x1b[A", ctrl).unwrap(), b"\x1b[1;5A");
    assert_eq!(mouser_render::modified_csi(b"\x1b[3~", ModifiersState::SHIFT).unwrap(), b"\x1b[3;2~");
    assert_eq!(mouser_render::modified_csi(b"\x1bOP", ModifiersState::ALT).unwrap(), b"\x1b[1;3P");
    assert_eq!(mouser_render::modified_csi(b"\x1b[A", ModifiersState::empty()), None);
}

fn ink_cols(buf: &[u32], w: usize, color: u32) -> Vec<usize> {
    let mut v: Vec<usize> = (0..buf.len()).filter(|i| buf[*i] & 0xFFFFFF != BG && buf[*i] != 0 && buf[*i] != color).map(|i| i % w).collect();
    v.sort();
    v.dedup();
    v
}

#[test]
fn combining_mark_draws_over_its_base_cell() {
    let mut p = CellPaint::plain(FG);
    p.comb[0] = '\u{301}';
    let (buf, w, f) = render(" ", vec![p], None, CursorShape::Block);
    let cols = ink_cols(&buf, w, 0);
    assert!(!cols.is_empty());
    let (lo, hi) = (PADDING as usize, (PADDING + f.cell_w) as usize);
    assert!(cols.iter().all(|&c| c >= lo && c < hi), "mark ink outside its cell: {cols:?}");
}

#[test]
fn italic_slants_the_glyph() {
    let mut f = FontStack::embedded(16.0);
    let up = f.glyph('l').clone();
    let it = f.glyph(mouser_render::text::italic_key('l')).clone();
    assert!(it.metrics.width > up.metrics.width);
    // Top row ink sits further right than bottom row ink.
    // Ink centroid of the top quarter sits right of the bottom quarter's, more than upright.
    let lean = |g: &mouser_render::text::Glyph| {
        let (w, h) = (g.metrics.width, g.metrics.height);
        let c = |rows: std::ops::Range<usize>| {
            let (mut sx, mut n) = (0f32, 0f32);
            for y in rows {
                for x in 0..w {
                    let a = g.bitmap[y * w + x] as f32;
                    sx += a * x as f32;
                    n += a;
                }
            }
            sx / n.max(1.0)
        };
        c(0..h / 4) - c(h - h / 4..h)
    };
    assert!(lean(&it) > lean(&up) + 1.0, "{} vs {}", lean(&it), lean(&up));
    let mut p = CellPaint::plain(FG);
    p.flags = paint::ITALIC;
    let (buf, _, _) = render("l", vec![p], None, CursorShape::Block);
    assert!(buf.iter().any(|&c| c & 0xFFFFFF != BG && c != 0));
}
