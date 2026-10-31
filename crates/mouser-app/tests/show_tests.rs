//! `mouser show` end to end against Mouser's own parser: capability query,
//! upload, placement and cursor movement.

use mouser_app::show::{Options, encode, fit_cells, parse_replies, prepare};
use mouser_core::parser::TerminalParser;
use mouser_core::scrollback::Scrollback;
use mouser_core::types::Size;
use std::io::Write;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Out(Arc<Mutex<Vec<u8>>>);
impl Write for Out {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn show(fmt: image::ImageFormat, w: u32, h: u32, cols: usize) -> TerminalParser<Out> {
    let out = Out::default();
    let mut t = TerminalParser::new(Size { cols, rows: 24 }, Arc::new(Scrollback::new(Default::default())), out.clone());
    t.cell_px = (10, 20);
    t.feed(b"\x1b[1;3H");
    // Same queries `mouser show` sends.
    t.feed(b"\x1bP=Fzcq;id=1\x1b\\\x1b[16t\x1b[18t\x1b[6n\x1b[c");
    let replies = String::from_utf8(std::mem::take(&mut *out.0.lock().unwrap())).unwrap();
    let info = parse_replies(&replies, false).expect("Mouser answers cq");
    assert_eq!((info.cell, info.cols, info.cursor_col), ((10, 20), cols as u32, 2));
    let mut data = Vec::new();
    image::RgbImage::from_fn(w, h, |x, _| image::Rgb([(x * 255 / w) as u8, 0, 255])).write_to(&mut std::io::Cursor::new(&mut data), fmt).unwrap();
    let p = prepare(&data, &info).unwrap();
    let (c, r) = fit_cells(p.w, p.h, &info, &Options::default());
    t.feed(encode(&p, 42, c, r, &info).as_bytes());
    let replies = String::from_utf8(std::mem::take(&mut *out.0.lock().unwrap())).unwrap();
    assert!(replies.contains("Fzgr;id=42,ok=1"), "{replies:?}");
    t
}

#[test]
fn jpeg_lands_fitted_to_the_width_and_cursor_moves_below() {
    // 1000x200 px = 100x10 natural cells; 40-col screen, cursor at col 2: 38 cols.
    let t = show(image::ImageFormat::Jpeg, 1000, 200, 40);
    let img = &t.fuzzball.gfx.images[&42];
    assert_eq!((img.width, img.height), (1000, 200));
    let pl = &t.fuzzball.gfx.placements[0];
    assert_eq!((pl.col, pl.cols, pl.rows, pl.row), (2, 38, 4, 0));
    assert_eq!((t.screen.cursor.x, t.screen.cursor.y), (0, 4), "line below the image");
}

#[test]
fn small_png_is_shown_at_natural_size() {
    let t = show(image::ImageFormat::Png, 30, 40, 80);
    let pl = &t.fuzzball.gfx.placements[0];
    assert_eq!((pl.cols, pl.rows), (3, 2));
    assert_eq!(t.fuzzball.gfx.images[&42].width, 30);
}

#[test]
fn large_rgba_upload_is_chunked_within_max_chunk() {
    let info = parse_replies("\x1bP=Fzcr;id=1,sub=gk,max_chunk=65536,g_mem=335544320,g_max_dim=10000,g_fmt=rgba:rgb\x1b\\", false).unwrap();
    let p = mouser_app::show::Payload { fmt: "rgba", w: 200, h: 200, bytes: vec![7; 200 * 200 * 4] };
    let s = encode(&p, 5, 10, 10, &info);
    let chunks = s.matches("Fzgu;").count();
    assert_eq!(chunks, (200 * 200 * 4usize).div_ceil(3) * 4 / 65536 + 1);
    let out = Out::default();
    let mut t = TerminalParser::new(Size { cols: 80, rows: 24 }, Arc::new(Scrollback::new(Default::default())), out.clone());
    t.feed(s.as_bytes());
    assert_eq!(t.fuzzball.gfx.images[&5].rgba.len(), 160_000);
}
