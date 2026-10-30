use mouser_core::graphics::{base64_decode, decode_png};
use mouser_core::parser::TerminalParser;
use mouser_core::scrollback::{Scrollback, ScrollbackConfig};
use mouser_core::types::Size;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn term() -> (TerminalParser<Sink>, Sink) {
    let sink = Sink::default();
    let mut p = TerminalParser::new(Size { cols: 40, rows: 10 }, Arc::new(Scrollback::new(ScrollbackConfig::default())), sink.clone());
    p.cell_px = (10, 20);
    (p, sink)
}
fn out(s: &Sink) -> String {
    String::from_utf8(std::mem::take(&mut *s.0.lock().unwrap())).unwrap()
}

fn b64(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

#[test]
fn base64_round_trip() {
    for data in [&b""[..], b"a", b"ab", b"abc", b"hello world!", &[0, 255, 128, 7]] {
        assert_eq!(base64_decode(b64(data).as_bytes()).unwrap(), data);
    }
    assert_eq!(base64_decode(b"aGk").unwrap(), b"hi");
    assert!(base64_decode(b"a$b").is_none());
}

#[test]
fn transmit_and_display_rgba_moves_cursor_and_replies() {
    let (mut p, sink) = term();
    let px = [255u8, 0, 0, 255].repeat(20 * 30); // 20x30 red
    p.feed(format!("ab\x1b_Ga=T,f=32,s=20,v=30,i=7;{}\x1b\\", b64(&px)).as_bytes());
    assert_eq!(out(&sink), "\x1b_Gi=7;OK\x1b\\");
    let img = &p.graphics.images[&7];
    assert_eq!((img.width, img.height), (20, 30));
    let pl = &p.graphics.placements[0];
    assert_eq!((pl.col, pl.row, pl.width, pl.height), (2, 0, 20, 30));
    // 20px = 2 cells wide, 30px = 2 rows: cursor after the image on its last row.
    assert_eq!((p.screen.cursor.x, p.screen.cursor.y), (4, 1));
}

#[test]
fn rgb_chunked_transfer_and_quiet() {
    let (mut p, sink) = term();
    let px = [1u8, 2, 3].repeat(4 * 4);
    let enc = b64(&px);
    let (a, b) = enc.split_at(8);
    p.feed(format!("\x1b_Ga=t,f=24,s=4,v=4,i=3,m=1;{a}\x1b\\").as_bytes());
    assert!(p.graphics.images.is_empty());
    p.feed(format!("\x1b_Gm=0;{b}\x1b\\").as_bytes());
    assert_eq!(out(&sink), "\x1b_Gi=3;OK\x1b\\");
    assert_eq!(&p.graphics.images[&3].rgba[..8], &[1, 2, 3, 255, 1, 2, 3, 255]);
    assert!(p.graphics.placements.is_empty());
    // q=1 suppresses OK, q=2 also errors.
    p.feed(format!("\x1b_Ga=t,f=24,s=4,v=4,i=4,q=1;{enc}\x1b\\").as_bytes());
    p.feed(b"\x1b_Ga=p,i=99,q=2\x1b\\");
    assert_eq!(out(&sink), "");
}

#[test]
fn put_with_placement_ids_cells_offsets_and_z() {
    let (mut p, sink) = term();
    let px = [0u8, 255, 0, 128].repeat(10 * 10);
    p.feed(format!("\x1b_Gi=1,f=32,s=10,v=10;{}\x1b\\", b64(&px)).as_bytes());
    out(&sink);
    p.feed(b"\x1b[3;5H\x1b_Ga=p,i=1,p=2,c=3,r=2,X=4,Y=5,z=-1,C=1\x1b\\");
    assert_eq!(out(&sink), "\x1b_Gi=1,p=2;OK\x1b\\");
    let pl = &p.graphics.placements[0];
    assert_eq!((pl.id, pl.col, pl.row, pl.width, pl.height, pl.x_off, pl.y_off, pl.z), (2, 4, 2, 30, 40, 4, 5, -1));
    // C=1: cursor stays put.
    assert_eq!((p.screen.cursor.x, p.screen.cursor.y), (4, 2));
    // Same placement id replaces it.
    p.feed(b"\x1b_Ga=p,i=1,p=2,x=2,y=2,w=4,h=4\x1b\\");
    assert_eq!(p.graphics.placements.len(), 1);
    assert_eq!(p.graphics.placements[0].src, (2, 2, 4, 4));
    p.feed(b"\x1b_Ga=p,i=1,p=3,C=1\x1b\\");
    assert_eq!(p.graphics.placements.len(), 2);
    // Missing image.
    out(&sink);
    p.feed(b"\x1b_Ga=p,i=42\x1b\\");
    assert!(out(&sink).starts_with("\x1b_Gi=42;ENOENT"));
}

#[test]
fn delete_actions() {
    let (mut p, _sink) = term();
    let px = b64(&[9u8; 4 * 4 * 4]);
    for i in 1..=3 {
        p.feed(format!("\x1b_Ga=T,i={i},s=4,v=4,q=2,C=1;{px}\x1b\\").as_bytes());
    }
    assert_eq!(p.graphics.placements.len(), 3);
    p.feed(b"\x1b_Ga=d,d=i,i=2\x1b\\");
    assert_eq!(p.graphics.placements.len(), 2);
    assert!(p.graphics.images.contains_key(&2), "lowercase keeps the data");
    p.feed(b"\x1b_Ga=d,d=I,i=1\x1b\\");
    assert!(!p.graphics.images.contains_key(&1), "uppercase frees it");
    p.feed(b"\x1b_Ga=d\x1b\\");
    assert!(p.graphics.placements.is_empty());
    assert_eq!(p.graphics.images.len(), 2);
}

#[test]
fn query_and_errors() {
    let (mut p, sink) = term();
    p.feed(format!("\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;{}\x1b\\", b64(&[0, 0, 0])).as_bytes());
    assert_eq!(out(&sink), "\x1b_Gi=31;OK\x1b\\");
    assert!(p.graphics.images.is_empty(), "query doesn't store");
    p.feed(b"\x1b_Gi=32,a=q,t=f,f=100;L3RtcC94\x1b\\");
    assert!(out(&sink).starts_with("\x1b_Gi=32;ENOTSUPPORTED"));
    p.feed(b"\x1b_Gi=33,a=t,f=32,s=10,v=10;AAAA\x1b\\");
    assert!(out(&sink).starts_with("\x1b_Gi=33;ENODATA"));
    p.feed(b"\x1b_Gi=34,a=t,f=100;AAAA\x1b\\");
    assert!(out(&sink).starts_with("\x1b_Gi=34;EBADPNG"));
    // No id: no reply.
    p.feed(b"\x1b_Ga=t,f=32,s=10,v=10;AAAA\x1b\\");
    assert_eq!(out(&sink), "");
    // Text around APC is unaffected, other APCs are ignored.
    p.feed(b"x\x1b_Xjunk\x1b\\y\x1b[1mz");
    assert!(p.screen.visible_lines()[0].starts_with("xyz"));
}

fn png_bytes(color: png::ColorType, depth: png::BitDepth, w: u32, h: u32, data: &[u8], palette: Option<&[u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(color);
        e.set_depth(depth);
        if let Some(pl) = palette {
            e.set_palette(pl.to_vec());
        }
        let mut wr = e.write_header().unwrap();
        wr.write_image_data(data).unwrap();
    }
    out
}

#[test]
fn png_formats_expand_to_rgba8() {
    use png::{BitDepth::*, ColorType::*};
    let rgba = decode_png(&png_bytes(Rgba, Eight, 1, 1, &[1, 2, 3, 4], None)).unwrap();
    assert_eq!(rgba, (1, 1, vec![1, 2, 3, 4]));
    assert_eq!(decode_png(&png_bytes(Rgb, Eight, 1, 1, &[5, 6, 7], None)).unwrap().2, [5, 6, 7, 255]);
    assert_eq!(decode_png(&png_bytes(Grayscale, Eight, 2, 1, &[10, 20], None)).unwrap().2, [10, 10, 10, 255, 20, 20, 20, 255]);
    assert_eq!(decode_png(&png_bytes(GrayscaleAlpha, Eight, 1, 1, &[9, 99], None)).unwrap().2, [9, 9, 9, 99]);
    // 16-bit is stripped to 8.
    assert_eq!(decode_png(&png_bytes(Rgb, Sixteen, 1, 1, &[0xAB, 0, 0x12, 0, 0xFF, 0xFF], None)).unwrap().2, [0xAB, 0x12, 0xFF, 255]);
    // Palette (2 entries, 1 bit per pixel).
    let pal = [255, 0, 0, 0, 0, 255];
    assert_eq!(decode_png(&png_bytes(Indexed, One, 2, 1, &[0b0100_0000], Some(&pal))).unwrap().2, [255, 0, 0, 255, 0, 0, 255, 255]);
}

#[test]
fn png_transmission_through_the_protocol() {
    let (mut p, sink) = term();
    let data = png_bytes(png::ColorType::Rgba, png::BitDepth::Eight, 2, 2, &[7u8; 16], None);
    p.feed(format!("\x1b_Ga=T,f=100,i=5,C=1;{}\x1b\\", b64(&data)).as_bytes());
    assert_eq!(out(&sink), "\x1b_Gi=5;OK\x1b\\");
    assert_eq!(p.graphics.images[&5].rgba.len(), 16);
}

#[test]
fn placements_follow_scrolling() {
    let (mut p, _s) = term();
    p.feed(format!("\x1b[5;1H\x1b_Ga=T,i=1,s=1,v=1,q=2,C=1;{}\x1b\\", b64(&[1, 1, 1, 1])).as_bytes());
    assert_eq!(p.graphics.placements[0].row, 4);
    p.feed(b"\x1b[10;1H\n\n\n");
    assert_eq!(p.lines_scrolled, 3);
    // Visible in a window starting 3 lines down: at screen row 1.
    let v = p.graphics.visible(p.lines_scrolled, 10, 20);
    assert_eq!(v[0].0.row - p.lines_scrolled, 1);
}
