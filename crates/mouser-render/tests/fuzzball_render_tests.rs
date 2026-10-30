//! Golden pixels for Fuzzball graphics: protocol bytes -> parser -> frame, on the
//! CPU canvas and through the display list the GPU renderer consumes.
use mouser_core::{Scrollback, Size, TerminalParser};
use mouser_render::text::{Canvas, DisplayList, DrawOp, FontStack};
use mouser_render::{Frame, PADDING, image_draws, render_frame};
use std::sync::Arc;

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

fn b64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            s.push(if i <= c.len() { A[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    s
}

fn render(place: &str) -> (Vec<u32>, DisplayList, usize, (i32, i32)) {
    render_after(place, "")
}

fn render_after(place: &str, after: &str) -> (Vec<u32>, DisplayList, usize, (i32, i32)) {
    let mut fonts = FontStack::embedded(15.0);
    let (cw, ch) = (fonts.cell_w, fonts.cell_h);
    let sb = Arc::new(Scrollback::new(Default::default()));
    let mut t = TerminalParser::new(Size { cols: 12, rows: 4 }, sb, std::io::sink());
    t.cell_px = (cw as u32, ch as u32);
    // 2x1 image: red, blue.
    let px: Vec<u8> = [RED, BLUE].concat();
    let p = b64(&px);
    t.feed(format!("\x1bP=Fzgu;id=1,fmt=rgba,w=2,h=1,size=8,len={}:{p}\x1b\\", p.len()).as_bytes());
    t.feed(format!("\x1b[2;3H\x1bP=Fzgp;{place}\x1b\\").as_bytes());
    t.feed(after.as_bytes());
    let images = image_draws(&t.fuzzball.gfx, false, t.lines_scrolled, 4);
    let lines = vec![String::new(); 4];
    let frame = Frame {
        lines: &lines,
        cursor: None,
        bg: 0,
        fg: 0xFFFFFF,
        ui: None,
        status: None,
        accent: None,
        cursor_color: None,
        images: &images,
        styles: &[],
        cursor_shape: Default::default(),
        tabs: None,
    };
    let (w, h) = ((cw * 12 + 2 * PADDING) as usize, (ch * 4 + 2 * PADDING) as usize);
    let mut cpu = vec![0u32; w * h];
    render_frame(&mut Canvas::new(&mut cpu, w, h), &mut fonts, &frame);
    let mut list = DisplayList::new(w, h);
    render_frame(&mut list, &mut fonts, &frame);
    (cpu, list, w, (cw, ch))
}

#[test]
fn placement_fills_its_cells_on_cpu() {
    let (buf, _, w, (cw, ch)) = render("id=1,pid=1,cols=4,rows=2");
    let at = |x: i32, y: i32| buf[y as usize * w + x as usize];
    let (x0, y0) = (PADDING + 2 * cw, PADDING + ch);
    // Left half red, right half blue, exactly 4x2 cells, nothing outside.
    assert_eq!(at(x0, y0), 0xFF0000);
    assert_eq!(at(x0 + 2 * cw - 1, y0 + 2 * ch - 1), 0xFF0000);
    assert_eq!(at(x0 + 2 * cw, y0), 0x0000FF);
    assert_eq!(at(x0 + 4 * cw - 1, y0 + 2 * ch - 1), 0x0000FF);
    assert_eq!(at(x0 - 1, y0), 0);
    assert_eq!(at(x0 + 4 * cw, y0), 0);
    assert_eq!(at(x0, y0 - 1), 0);
    assert_eq!(at(x0, y0 + 2 * ch), 0);
}

#[test]
fn source_rectangle_selects_pixels() {
    let (buf, _, w, (cw, ch)) = render("id=1,pid=1,cols=2,rows=1,sx=1,sw=1");
    let (x0, y0) = (PADDING + 2 * cw, PADDING + ch);
    assert_eq!(buf[y0 as usize * w + x0 as usize], 0x0000FF);
    assert_eq!(buf[y0 as usize * w + (x0 + 2 * cw - 1) as usize], 0x0000FF);
}

#[test]
fn gpu_display_list_matches_cpu() {
    let (cpu, list, w, (cw, ch)) = render("id=1,pid=1,cols=4,rows=2");
    // The GPU renderer draws exactly the recorded image op: same rectangle.
    let img = list
        .ops
        .iter()
        .find_map(|o| match o {
            DrawOp::Image { index, x, y } => Some((*index, *x, *y)),
            _ => None,
        })
        .expect("image op recorded");
    assert_eq!((img.1, img.2), (PADDING + 2 * cw, PADDING + ch));
    let d = &list.images[img.0];
    assert_eq!((d.width, d.height, d.src), ((4 * cw) as u32, (2 * ch) as u32, (0, 0, 2, 1)));
    // And replaying the list gives identical pixels.
    let mut fonts = FontStack::embedded(15.0);
    let h = cpu.len() / w;
    let mut replayed = vec![0u32; cpu.len()];
    list.replay(&mut Canvas::new(&mut replayed, w, h), &mut fonts);
    assert!(cpu == replayed);
}

#[test]
fn placement_pixels_follow_a_scrolled_region() {
    // Scroll rows 1..4 up by one: the image moves from row 1 to row 0.
    let (buf, _, w, (cw, ch)) = render_after("id=1,pid=1,cols=4,rows=1,move=0", "\x1b[1;4r\x1b[S");
    let at = |x: i32, y: i32| buf[y as usize * w + x as usize];
    let x0 = PADDING + 2 * cw;
    assert_eq!(at(x0, PADDING), 0xFF0000);
    assert_eq!(at(x0, PADDING + ch), 0, "old row is blank");
}

#[test]
fn evicted_placement_draws_nothing() {
    // Scrolled into the scrollback: nothing is drawn on the live screen.
    let (buf, list, _, _) = render_after("id=1,pid=1,cols=4,rows=1,move=0", "\x1b[2S");
    assert!(buf.iter().all(|&p| p == 0));
    assert!(!list.ops.iter().any(|o| matches!(o, DrawOp::Image { .. })));
}
