use mouser_render::boxdraw::Geometry;
use mouser_render::text::{Canvas, DisplayList, Draw, DrawOp, FontStack};

const ROWS: [&str; 7] = ["┌──┬──┐╔══╦══╗", "│  │  │║  ║  ║", "├──┼──┤╠══╬══╣", "└──┴──┘╚══╩══╝", "┏━━┳━━┓╶─╴ ", "┃  ┃  ┃ ═ ", "┗━━┻━━┛    "];

fn draw(c: &mut dyn Draw, fonts: &mut FontStack) {
    for (i, row) in ROWS.iter().enumerate() {
        c.draw_text(fonts, 0, i as i32 * fonts.cell_h, row, 0xFFFFFF);
    }
}

/// What the GPU renderer does with a display list: integer quads, nearest
/// sampling of each glyph's bitmap from the atlas.
fn rasterise(list: &DisplayList, fonts: &mut FontStack, w: usize, h: usize) -> Vec<u32> {
    let mut out = vec![0u32; w * h];
    for op in &list.ops {
        if let DrawOp::Glyph { x, y, ch, .. } = *op {
            let g = fonts.glyph(ch);
            for gy in 0..g.metrics.height {
                for gx in 0..g.metrics.width {
                    let (px, py) = (x + gx as i32, y + gy as i32);
                    if g.bitmap[gy * g.metrics.width + gx] == 255 && px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                        out[py as usize * w + px as usize] = 0xFFFFFF;
                    }
                }
            }
        }
    }
    out
}

#[test]
fn cpu_and_gpu_paths_place_box_lines_identically() {
    for size in [11.0, 13.0, 14.5, 16.0, 19.0, 23.0] {
        let mut fonts = FontStack::embedded(size);
        let (cw, ch) = (fonts.cell_w, fonts.cell_h);
        let (w, h) = ((cw * 16) as usize, (ch * ROWS.len() as i32) as usize);
        let mut cpu = vec![0u32; w * h];
        draw(&mut Canvas { buf: &mut cpu, width: w, height: h }, &mut fonts);
        let mut list = DisplayList::new(w, h);
        draw(&mut list, &mut fonts);
        let gpu = rasterise(&list, &mut fonts, w, h);
        // Box glyphs are fully opaque, so only compare full-coverage pixels.
        let solid = |b: &[u32]| b.iter().map(|&p| p == 0xFFFFFF).collect::<Vec<_>>();
        assert_eq!(solid(&cpu), solid(&gpu), "CPU and GPU differ at size {size}");

        let lit = |x: i32, y: i32| cpu[(y as usize) * w + x as usize] == 0xFFFFFF;
        let g = Geometry::new(cw, ch);
        let hy = Geometry::start(ch, g.light);
        let vx = Geometry::start(cw, g.light);
        // Top, middle and bottom edges of the light box are continuous.
        for row in [0, 2, 3] {
            let y = row * ch + hy;
            assert!((vx..cw * 6 + vx).all(|x| lit(x, y)), "row {row} has gaps at size {size}");
        }
        // Left, middle and right verticals are continuous through all four rows.
        for col in [0, 3, 6] {
            let x = col * cw + vx;
            assert!((hy..3 * ch + hy).all(|y| lit(x, y)), "col {col} has gaps at size {size}");
        }
        // Double box: the outer stroke runs unbroken; the inner one breaks only at the tee.
        let (ay, by) = g.double(ch);
        assert!((8 * cw..13 * cw).all(|x| lit(x, ay)), "double outer has gaps at size {size}");
        assert!((8 * cw..10 * cw).chain(11 * cw..13 * cw).all(|x| lit(x, by)), "double inner has gaps at size {size}");
    }
}
