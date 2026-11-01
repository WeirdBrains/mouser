//! `mouser show` end to end against Mouser's own parser: capability query,
//! upload, placement and cursor movement.

use mouser_app::show::{Options, Proto, encode, encode_kitty, encode_placeholders, fills, fit_cells, parse_replies, placeholder_id, prepare, query};
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
    let mut info = parse_replies(&replies, false).expect("Mouser answers cq");
    assert_eq!((info.cell, info.cols, info.cursor_col), ((10, 20), cols as u32, 2));
    // As `mouser show` does: fitting the width starts on a new line.
    t.feed(b"\r\n");
    info.cursor_col = 0;
    assert!(info.fit_w, "Mouser advertises g_fit=w");
    let mut data = Vec::new();
    image::RgbImage::from_fn(w, h, |x, _| image::Rgb([(x * 255 / w) as u8, 0, 255])).write_to(&mut std::io::Cursor::new(&mut data), fmt).unwrap();
    let p = prepare(&data, &info).unwrap();
    let (c, r) = fit_cells(p.w, p.h, &info, &Options::default());
    t.feed(encode(&p, 42, c, r, fills(&Options::default()), &info).as_bytes());
    let replies = String::from_utf8(std::mem::take(&mut *out.0.lock().unwrap())).unwrap();
    assert!(replies.contains("Fzgr;id=42,ok=1"), "{replies:?}");
    t
}

#[test]
fn jpeg_lands_fitted_to_the_width_and_cursor_moves_below() {
    // 1000x200 px = 100x10 natural cells on a 40-col screen: 40x4 cells from col 0
    // on the line below the cursor.
    let t = show(image::ImageFormat::Jpeg, 1000, 200, 40);
    let img = &t.fuzzball.gfx.images[&42];
    assert_eq!((img.width, img.height), (1000, 200));
    let pl = &t.fuzzball.gfx.placements[0];
    assert_eq!((pl.col, pl.cols, pl.rows, pl.row, pl.fill), (0, 40, 4, 1, true));
    assert_eq!((t.screen.cursor.x, t.screen.cursor.y), (0, 5), "line below the image");
}

#[test]
fn small_png_is_scaled_up_to_the_width() {
    // 30x40 px (3x2 natural cells) on 80 columns: 80 cols, 800x1067 px = 54 rows.
    let mut t = show(image::ImageFormat::Png, 30, 40, 80);
    let pl = &t.fuzzball.gfx.placements[0];
    assert_eq!((pl.col, pl.cols, pl.rows), (0, 80, 54));
    // It follows the width on resize, both ways.
    t.resize(Size { cols: 40, rows: 24 });
    let pl = &t.fuzzball.gfx.placements[0];
    assert_eq!((pl.col, pl.cols, pl.rows), (0, 40, 27));
    assert_eq!(pl.pixel_size((10, 20)), (400, 540));
    t.resize(Size { cols: 120, rows: 24 });
    let pl = &t.fuzzball.gfx.placements[0];
    assert_eq!((pl.col, pl.cols, pl.rows), (0, 120, 81));
    assert_eq!(t.fuzzball.gfx.images[&42].width, 30);
}

#[test]
fn large_rgba_upload_is_chunked_within_max_chunk() {
    let info = parse_replies("\x1bP=Fzcr;id=1,sub=gk,max_chunk=65536,g_mem=335544320,g_max_dim=10000,g_fmt=rgba:rgb\x1b\\", false).unwrap();
    let p = mouser_app::show::Payload { fmt: "rgba", w: 200, h: 200, bytes: vec![7; 200 * 200 * 4] };
    let s = encode(&p, 5, 10, 10, false, &info);
    let chunks = s.matches("Fzgu;").count();
    assert_eq!(chunks, (200 * 200 * 4usize).div_ceil(3) * 4 / 65536 + 1);
    let out = Out::default();
    let mut t = TerminalParser::new(Size { cols: 80, rows: 24 }, Arc::new(Scrollback::new(Default::default())), out.clone());
    t.feed(s.as_bytes());
    assert_eq!(t.fuzzball.gfx.images[&5].rgba.len(), 160_000);
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut data = Vec::new();
    image::RgbaImage::from_pixel(w, h, image::Rgba([0, 128, 255, 255]))
        .write_to(&mut std::io::Cursor::new(&mut data), image::ImageFormat::Png)
        .unwrap();
    data
}

#[test]
fn mouser_answers_both_queries_and_fuzzball_wins() {
    let out = Out::default();
    let mut t = TerminalParser::new(Size { cols: 80, rows: 24 }, Arc::new(Scrollback::new(Default::default())), out.clone());
    t.feed(query(false).as_bytes());
    let replies = String::from_utf8(std::mem::take(&mut *out.0.lock().unwrap())).unwrap();
    assert!(replies.contains("\x1b_Gi=31;OK"), "{replies:?}");
    assert_eq!(parse_replies(&replies, false).unwrap().proto, Proto::Fuzzball);
    assert!(t.kitty_gfx.gfx.images.is_empty(), "the query stores nothing");
}

#[test]
fn kitty_fallback_places_at_the_cursor() {
    let out = Out::default();
    let mut t = TerminalParser::new(Size { cols: 80, rows: 24 }, Arc::new(Scrollback::new(Default::default())), out.clone());
    t.cell_px = (10, 20);
    // A Kitty-only terminal: no Fuzzball reply.
    let info = parse_replies("\x1b_Gi=31;OK\x1b\\\x1b[6;20;10t\x1b[8;24;80t\x1b[1;1R\x1b[?62c", false).unwrap();
    let p = prepare(&png(100, 40), &info).unwrap();
    assert_eq!(p.fmt, "png");
    let (c, r) = fit_cells(p.w, p.h, &info, &Options::default());
    t.feed(encode_kitty(&p, 77, c, r, false).as_bytes());
    assert_eq!(String::from_utf8(std::mem::take(&mut *out.0.lock().unwrap())).unwrap(), "", "q=1: no OK");
    let pl = &t.kitty_gfx.gfx.placements[0];
    // 100x40 px scaled up to the 80-column width: 80x16 cells.
    assert_eq!((pl.image_id, pl.cols, pl.rows), (77, 80, 16));
    assert_eq!((t.screen.cursor.x, t.screen.cursor.y), (0, 16), "below the image");
}

#[test]
fn placeholders_show_the_image_as_text() {
    let out = Out::default();
    let mut t = TerminalParser::new(Size { cols: 80, rows: 24 }, Arc::new(Scrollback::new(Default::default())), out.clone());
    t.cell_px = (10, 20);
    let info = parse_replies("\x1b_Gi=31;OK\x1b\\\x1b[6;20;10t\x1b[8;24;80t\x1b[1;1R\x1b[?62c", true).unwrap();
    assert_eq!(info.proto, Proto::KittyPlaceholders);
    let p = prepare(&png(100, 40), &info).unwrap();
    let (c, r) = fit_cells(p.w, p.h, &info, &Options::default());
    // What tmux hands the outer terminal: the unwrapped commands and the text.
    t.feed(encode_placeholders(&p, placeholder_id(77_777), c, r, false).as_bytes());
    assert_eq!(String::from_utf8(std::mem::take(&mut *out.0.lock().unwrap())).unwrap(), "", "q=2: silent");
    assert!(t.kitty_gfx.gfx.placements[0].virt);
    assert_eq!((t.screen.cursor.x, t.screen.cursor.y), (0, 16), "80x16 placeholder cells");
    let screen = &t.screen;
    let row = |i: usize| (i < 24).then(|| screen.row(i));
    let d = t.kitty_gfx.placeholder_draws(24, &row, (10, 20));
    assert_eq!(d.len(), 16, "one run per row");
    assert_eq!((d[0].width, d[0].height, d[1].row), (800, 20, 1));
}
