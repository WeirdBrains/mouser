use mouser_render::text::{Canvas, FontStack};

/// A run of box-drawing characters must be continuous across cells (no dashes).
#[test]
fn box_lines_are_continuous_across_cells() {
    let mut fonts = FontStack::embedded(16.0);
    let (cw, ch) = (fonts.cell_w, fonts.cell_h);
    let (w, h) = ((cw * 6) as usize, (ch * 3) as usize);
    let mut buf = vec![0u32; w * h];
    let mut c = Canvas { buf: &mut buf, width: w, height: h };
    c.draw_text(&mut fonts, 0, 0, "┌────┐", 0xFFFFFF);
    c.draw_text(&mut fonts, 0, ch, "│    │", 0xFFFFFF);
    c.draw_text(&mut fonts, 0, 2 * ch, "└────┘", 0xFFFFFF);
    let lit = |x: i32, y: i32| buf[(y as usize) * w + x as usize] != 0;
    // Bottom edge: every column between the corners' centres is inked.
    let y = 2 * ch + ch / 2;
    assert!((cw / 2..cw * 6 - cw / 2).all(|x| lit(x, y)), "bottom edge has gaps");
    let y = ch / 2;
    assert!((cw / 2..cw * 6 - cw / 2).all(|x| lit(x, y)), "top edge has gaps");
    // Left edge: every row between the corners' centres is inked.
    let x = cw / 2;
    assert!((ch / 2..ch * 3 - ch / 2).all(|y| lit(x, y)), "left edge has gaps");
}
