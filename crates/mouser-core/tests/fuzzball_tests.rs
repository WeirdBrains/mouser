//! Fuzzball protocol conformance: framing, capability query, graphics,
//! keyboard mode, errors and limits (docs/protocol/).

use mouser_core::fuzzball::{self, KeyAction, KeyEvent, KeyName, encode_key};
use mouser_core::parser::TerminalParser;
use mouser_core::scrollback::{Scrollback, ScrollbackConfig};
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
impl Out {
    fn take(&self) -> String {
        String::from_utf8_lossy(&std::mem::take(&mut *self.0.lock().unwrap())).to_string()
    }
}

fn term() -> (TerminalParser<Out>, Out) {
    let out = Out::default();
    let sb = Arc::new(Scrollback::new(ScrollbackConfig::default()));
    let mut t = TerminalParser::new(Size { cols: 20, rows: 6 }, sb, out.clone());
    t.cell_px = (10, 20);
    (t, out)
}

fn b64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

fn msg(body: &str) -> Vec<u8> {
    format!("\x1bP=F{body}\x1b\\").into_bytes()
}

fn upload(id: u32, w: u32, h: u32, rgba: &[u8], extra: &str) -> Vec<u8> {
    let p = b64(rgba);
    msg(&format!("zgu;id={id},fmt=rgba,w={w},h={h},size={},enc=b64,len={}{extra}:{p}", rgba.len(), p.len()))
}

// README §4: capability negotiation.
#[test]
fn capability_query_then_da1() {
    let (mut t, out) = term();
    t.feed(b"\x1bP=Fzcq;id=1\x1b\\\x1b[c");
    let r = out.take();
    let (fz, da) = r.split_at(r.find("\x1b[?").unwrap());
    assert!(fz.starts_with("\x1bP=Fzcr;id=1,v=0,sub=gk,raw=0,max_chunk=65536,"), "{fz}");
    for k in ["g_mem=335544320", "g_max_dim=10000", "g_fmt=rgba:rgb:png", "g_max_place=4096", "k=1", "k_ime=0", "k_release=1"] {
        assert!(fz.contains(k), "{k} in {fz}");
    }
    assert!(fz.ends_with("\x1b\\"));
    assert!(da.starts_with("\x1b[?62;"));
}

#[test]
fn xtgettcap_advertises_fuzzball() {
    let (mut t, out) = term();
    t.feed(b"\x1bP+q467a\x1b\\");
    assert_eq!(out.take(), "\x1bP1+r467a=30\x1b\\");
}

// README §5: replies only with id, quiet levels, unknown verbs.
#[test]
fn replies_need_id_and_honour_quiet() {
    let (mut t, out) = term();
    t.feed(&msg("zcq"));
    assert_eq!(out.take(), "");
    t.feed(&msg("zgx;id=3"));
    assert_eq!(out.take(), "\x1bP=Fzgr;id=3,ok=0,err=ENOTSUP,msg=unknown subsystem or verb\x1b\\");
    t.feed(&msg("zgx"));
    assert_eq!(out.take(), "");
    t.feed(&upload(1, 1, 1, &[1, 2, 3, 4], ",quiet=1"));
    assert_eq!(out.take(), "");
    t.feed(&msg("zgp;id=9,quiet=1"));
    assert!(out.take().contains("err=ENOENT"));
    t.feed(&msg("zgp;id=9,quiet=2"));
    assert_eq!(out.take(), "");
}

#[test]
fn unknown_params_are_ignored() {
    let (mut t, out) = term();
    t.feed(&msg("zcq;id=2,future=1"));
    assert!(out.take().starts_with("\x1bP=Fzcr;id=2,v=0"));
}

// graphics.md §2.1 upload.
#[test]
fn upload_rgba_and_reply() {
    let (mut t, out) = term();
    t.feed(&upload(7, 2, 1, &[255, 0, 0, 255, 0, 0, 255, 255], ""));
    assert_eq!(out.take(), "\x1bP=Fzgr;id=7,ok=1,w=2,h=1\x1b\\");
    let img = &t.fuzzball.gfx.images[&7];
    assert_eq!((img.width, img.height), (2, 1));
    assert_eq!(&img.rgba[..], &[255, 0, 0, 255, 0, 0, 255, 255]);
}

#[test]
fn spec_example_payload() {
    let (mut t, out) = term();
    t.feed(&msg("zgu;id=1,fmt=rgba,w=2,h=1,size=8,enc=b64,len=12:/wAA/wAA//8="));
    assert_eq!(out.take(), "\x1bP=Fzgr;id=1,ok=1,w=2,h=1\x1b\\");
    assert_eq!(&t.fuzzball.gfx.images[&1].rgba[..], &[255, 0, 0, 255, 0, 0, 255, 255]);
}

#[test]
fn upload_rgb_expands_alpha() {
    let (mut t, _) = term();
    let p = b64(&[1, 2, 3]);
    t.feed(&msg(&format!("zgu;id=2,fmt=rgb,w=1,h=1,size=3,len={}:{p}", p.len())));
    assert_eq!(&t.fuzzball.gfx.images[&2].rgba[..], &[1, 2, 3, 255]);
}

#[test]
fn upload_png() {
    let mut png_bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png_bytes, 2, 2);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[9, 8, 7, 9, 8, 7, 9, 8, 7, 9, 8, 7]).unwrap();
    }
    let (mut t, out) = term();
    let p = b64(&png_bytes);
    t.feed(&msg(&format!("zgu;id=3,fmt=png,size={},len={}:{p}", png_bytes.len(), p.len())));
    assert_eq!(out.take(), "\x1bP=Fzgr;id=3,ok=1,w=2,h=2\x1b\\");
    assert_eq!(&t.fuzzball.gfx.images[&3].rgba[..4], &[9, 8, 7, 255]);
}

#[test]
fn chunked_upload_any_split() {
    let rgba: Vec<u8> = (0..64u32 * 4).map(|i| i as u8).collect();
    let p = b64(&rgba);
    for cut in [4, 100, 200, 340] {
        let (mut t, out) = term();
        let (a, b) = p.split_at(cut);
        t.feed(&msg(&format!("zgu;id=5,fmt=rgba,w=8,h=8,size=256,enc=b64,len={},more=1:{a}", a.len())));
        assert_eq!(out.take(), "", "no reply mid-upload");
        t.feed(&msg(&format!("zgu;id=5,enc=b64,len={},more=0:{b}", b.len())));
        assert_eq!(out.take(), "\x1bP=Fzgr;id=5,ok=1,w=8,h=8\x1b\\");
        assert_eq!(&t.fuzzball.gfx.images[&5].rgba[..], &rgba[..]);
    }
}

#[test]
fn byte_by_byte_feed_matches() {
    let (mut t, out) = term();
    for b in upload(4, 1, 1, &[1, 2, 3, 4], "") {
        t.feed(&[b]);
    }
    assert_eq!(out.take(), "\x1bP=Fzgr;id=4,ok=1,w=1,h=1\x1b\\");
}

#[test]
fn chunk_for_other_id_aborts() {
    let (mut t, out) = term();
    let p = b64(&[0; 8]);
    let (a, _) = p.split_at(4);
    t.feed(&msg(&format!("zgu;id=5,fmt=rgba,w=2,h=1,size=8,len=4,more=1:{a}")));
    t.feed(&msg("zgu;id=6,len=4,more=0:AAAA"));
    assert!(out.take().contains("id=6,ok=0,err=EPROTO"));
    assert!(t.fuzzball.gfx.images.is_empty());
}

#[test]
fn upload_errors() {
    let (mut t, out) = term();
    let cases = [
        ("zgu;id=1,fmt=rgba,w=1,h=1,size=4,len=3:AAAA", "EPROTO"),
        ("zgu;id=1,fmt=rgba,w=1,h=1,size=4,enc=raw,len=4:AAAA", "EPERM"),
        ("zgu;id=1,fmt=jpeg,size=4,len=8:AAAAAAAA", "ENOTSUP"),
        ("zgu;id=1,fmt=rgba,size=4,len=8:AAAAAAAA", "EINVAL"),
        ("zgu;id=1,fmt=rgba,w=1,h=1,size=5,len=8:AAAAAAAA", "EINVAL"),
        ("zgu;id=1,fmt=rgba,w=20000,h=1,size=80000,len=4:AAAA", "ENOMEM"),
        ("zgu;id=1,fmt=rgba,w=1,h=1,size=4,len=4:!!!!", "EINVAL"),
        ("zgu;id=1,fmt=rgba,w=1,h=1,size=4,cs=p3,len=8:AAAAAAAA", "ENOTSUP"),
        ("zgu;id=1,fmt=png,size=6,len=8:AAAAAAAA", "EINVAL"),
        ("zgu;id=1,fmt=rgba,w=1,h=1,size=4,len=8:AAAAAAAAAAAA", "EPROTO"),
    ];
    for (m, code) in cases {
        t.feed(&msg(m));
        let r = out.take();
        assert!(r.contains(&format!("err={code}")), "{m} -> {r}");
    }
    assert!(t.fuzzball.gfx.images.is_empty());
}

#[test]
fn oversize_message_is_dropped() {
    let (mut t, out) = term();
    let big = "A".repeat(fuzzball::MAX_MESSAGE + 10);
    t.feed(&msg(&format!("zgu;id=1,fmt=rgba,w=1,h=1,size=4,len={}:{big}", big.len())));
    assert_eq!(out.take(), "");
    // The parser recovers.
    t.feed(&msg("zcq;id=1"));
    assert!(out.take().starts_with("\x1bP=Fzcr;id=1"));
}

#[test]
fn reupload_replaces_image_and_placements() {
    let (mut t, _) = term();
    t.feed(&upload(1, 1, 1, &[1, 1, 1, 1], ""));
    t.feed(&msg("zgp;id=1,pid=1"));
    assert_eq!(t.fuzzball.gfx.placements.len(), 1);
    t.feed(&upload(1, 1, 1, &[2, 2, 2, 2], ""));
    assert!(t.fuzzball.gfx.placements.is_empty());
    assert_eq!(t.fuzzball.gfx.images[&1].rgba[0], 2);
}

// graphics.md §2.2 place.
#[test]
fn place_sizes_and_cursor_move() {
    let (mut t, out) = term();
    // 25x30 px image, cell 10x20.
    t.feed(&upload(1, 25, 30, &vec![0; 25 * 30 * 4], ",quiet=1"));
    t.feed(b"\x1b[2;3H");
    t.feed(&msg("zgp;id=1,pid=1,id=1"));
    let p = t.fuzzball.gfx.placements[0].clone();
    assert_eq!((p.col, p.row, p.cols, p.rows), (2, 1, 3, 2));
    assert_eq!(p.pixel_size((10, 20)), (30, 40));
    // Cursor after the last column on the last row.
    assert_eq!((t.screen.cursor.x, t.screen.cursor.y), (5, 2));
    out.take();
    t.feed(&msg("zgp;id=1,pid=2,cols=4,move=0"));
    let p = &t.fuzzball.gfx.placements[1];
    // rows from aspect: 4*10*30/25 = 48 px -> 3 rows (rounded up).
    assert_eq!((p.cols, p.rows), (4, 3));
    assert_eq!((t.screen.cursor.x, t.screen.cursor.y), (5, 2));
    t.feed(&msg("zgp;id=1,pid=3,rows=1,move=0,sx=5,sy=5,sw=10,sh=10,z=-1"));
    let p = &t.fuzzball.gfx.placements[2];
    assert_eq!((p.cols, p.rows, p.src, p.z), (2, 1, (5, 5, 10, 10), -1));
}

#[test]
fn place_existing_pid_moves_it() {
    let (mut t, _) = term();
    t.feed(&upload(1, 1, 1, &[0; 4], ""));
    t.feed(&msg("zgp;id=1,pid=1,move=0"));
    t.feed(b"\x1b[4;4H");
    t.feed(&msg("zgp;id=1,pid=1,move=0"));
    assert_eq!(t.fuzzball.gfx.placements.len(), 1);
    assert_eq!(t.fuzzball.gfx.placements[0].col, 3);
}

#[test]
fn place_errors() {
    let (mut t, out) = term();
    t.feed(&upload(1, 4, 4, &[0; 64], ",quiet=1"));
    for (m, code) in
        [("zgp;id=2", "ENOENT"), ("zgp", "EINVAL"), ("zgp;id=1,sx=9", "EINVAL"), ("zgp;id=1,z=5", "EINVAL"), ("zgp;id=1,cols=5000", "EINVAL")]
    {
        t.feed(&msg(&format!("{m}{}", if m.contains("id=") { "" } else { ";id=0" })));
        let r = out.take();
        if m == "zgp" {
            assert_eq!(r, "", "id=0 gets no reply");
        } else {
            assert!(r.contains(&format!("err={code}")), "{m} -> {r}");
        }
    }
}

#[test]
fn placement_limit() {
    let (mut t, out) = term();
    t.feed(&upload(1, 1, 1, &[0; 4], ",quiet=1"));
    for pid in 1..=mouser_core::graphics::MAX_PLACEMENTS as u32 {
        t.feed(&msg(&format!("zgp;id=1,pid={pid},move=0")));
    }
    out.take();
    t.feed(&msg("zgp;id=1,pid=99999,move=0"));
    assert!(out.take().contains("err=ENOMEM"));
}

// graphics.md §2.3 delete.
#[test]
fn delete_variants() {
    let (mut t, out) = term();
    t.feed(&upload(1, 10, 20, &vec![0; 800], ",quiet=1"));
    t.feed(&upload(2, 10, 20, &vec![0; 800], ",quiet=1"));
    t.feed(&msg("zgp;id=1,pid=1,move=0"));
    t.feed(&msg("zgp;id=1,pid=2,move=0"));
    t.feed(b"\x1b[3;5H");
    t.feed(&msg("zgp;id=2,pid=1,move=0"));
    t.feed(&msg("zgd;what=image,id=1,pid=2"));
    assert_eq!(t.fuzzball.gfx.placements.len(), 2);
    t.feed(&msg("zgd;what=cell"));
    assert_eq!(t.fuzzball.gfx.placements.len(), 1);
    assert!(t.fuzzball.gfx.images.contains_key(&2), "no free=1");
    t.feed(&msg("zgd;what=image,id=1,free=1"));
    assert!(!t.fuzzball.gfx.images.contains_key(&1));
    t.feed(&msg("zgp;id=2,pid=1,move=0"));
    t.feed(&msg("zgd;what=all,free=1"));
    assert!(t.fuzzball.gfx.placements.is_empty() && t.fuzzball.gfx.images.is_empty());
    t.feed(&msg("zgd;what=image,id=1;id=4"));
    t.feed(&msg("zgd;id=4,what=image,id=1"));
    t.feed(&msg("zgd;id=4,what=nope"));
    let r = out.take();
    assert!(r.contains("err=ENOTSUP"), "{r}");
}

// graphics.md §2.4 query.
#[test]
fn graphics_query() {
    let (mut t, out) = term();
    t.feed(&upload(1, 1, 1, &[0; 4], ",quiet=1"));
    t.feed(&msg("zgq;id=9"));
    assert_eq!(out.take(), "\x1bP=Fzgr;id=9,ok=1,mem_used=4,mem_max=335544320,images=1,cell_w=10,cell_h=20\x1b\\");
}

// graphics.md §4: interaction with text.
#[test]
fn erase_removes_placements() {
    let (mut t, _) = term();
    t.feed(&upload(1, 10, 20, &vec![0; 800], ",quiet=1"));
    t.feed(b"\x1b[2;2H");
    t.feed(&msg("zgp;id=1,pid=1,cols=2,rows=2,move=0"));
    t.feed(b"\x1b[1;1H\x1b[K");
    assert_eq!(t.fuzzball.gfx.placements.len(), 1, "EL on another row keeps it");
    t.feed(b"\x1b[3;3H\x1b[X");
    assert!(t.fuzzball.gfx.placements.is_empty(), "ECH over a cell removes it");
    t.feed(&msg("zgp;id=1,pid=1,move=0"));
    t.feed(b"\x1b[2J");
    assert!(t.fuzzball.gfx.placements.is_empty(), "ED 2 removes it");
}

#[test]
fn screens_have_own_placements_and_ris_clears() {
    let (mut t, _) = term();
    t.feed(&upload(1, 1, 1, &[0; 4], ",quiet=1"));
    t.feed(b"\x1b[?1049h");
    t.feed(&msg("zgp;id=1,pid=1,move=0"));
    assert_eq!(t.fuzzball.gfx.visible(true, 0, 6).len(), 1);
    assert_eq!(t.fuzzball.gfx.visible(false, 0, 6).len(), 0);
    t.feed(b"\x1b[?1049l");
    assert!(t.fuzzball.gfx.placements.is_empty());
    assert!(t.fuzzball.gfx.images.contains_key(&1), "images are shared");
    t.feed(&msg("zgp;id=1,pid=1,move=0"));
    t.feed(b"\x1bc");
    assert!(t.fuzzball.gfx.images.is_empty() && t.fuzzball.gfx.placements.is_empty());
}

#[test]
fn placements_scroll_with_text() {
    let (mut t, _) = term();
    t.feed(&upload(1, 1, 1, &[0; 4], ",quiet=1"));
    t.feed(b"\x1b[6;1H");
    t.feed(&msg("zgp;id=1,pid=1,move=0"));
    let row = t.fuzzball.gfx.placements[0].row;
    t.feed(b"\n\n");
    // Absolute row stays; the viewport top moved down by two.
    assert_eq!(t.fuzzball.gfx.placements[0].row, row);
    assert_eq!(t.lines_scrolled, 2);
}

// keyboard.md §1 mode negotiation.
#[test]
fn keyboard_mode_per_screen() {
    let (mut t, out) = term();
    t.feed(&msg("zks;on=1,id=1"));
    assert_eq!(out.take(), "\x1bP=Fzkr;id=1,ok=1,on=1\x1b\\");
    assert!(t.fuzzball.keyboard(false));
    t.feed(b"\x1b[?1049h");
    assert!(!t.fuzzball.keyboard(true));
    t.feed(&msg("zks;on=1"));
    t.feed(&msg("zkq;id=2"));
    assert_eq!(out.take(), "\x1bP=Fzkr;id=2,ok=1,on=1\x1b\\");
    t.feed(b"\x1b[?1049l");
    assert!(t.fuzzball.keyboard(false), "main screen keeps its own state");
    t.feed(b"\x1b[?1049h");
    assert!(!t.fuzzball.keyboard(true), "leaving the alt screen turned its mode off");
    t.feed(b"\x1b[?1049l");
    t.feed(&msg("zks;on=1,v=7,id=3"));
    assert!(out.take().contains("err=EVERSION"));
    t.feed(&msg("zks;on=2,id=4"));
    assert!(out.take().contains("err=EINVAL"));
    t.feed(b"\x1bc");
    assert!(!t.fuzzball.keyboard(false), "RIS turns it off");
}

// keyboard.md §2 event encoding (spec examples).
fn ev(action: KeyAction, code: &str, key: KeyName, mods: u8, text: Option<&str>) -> String {
    encode_key(&KeyEvent { action, code: code.into(), key, mods, locks: 0, text: text.map(Into::into) })
}

#[test]
fn key_event_examples() {
    use KeyAction::*;
    let c = KeyName::Char;
    let n = |s: &str| KeyName::Named(s.into());
    assert_eq!(ev(Press, "KeyA", c('a'), 0, Some("a")), "\x1bP=Fzke;t=p,code=KeyA,key=U+0061,text=61\x1b\\");
    assert_eq!(ev(Press, "KeyA", c('a'), 1, Some("A")), "\x1bP=Fzke;t=p,code=KeyA,key=U+0061,mods=1,text=41\x1b\\");
    assert_eq!(ev(Release, "KeyA", c('a'), 0, Some("a")), "\x1bP=Fzke;t=u,code=KeyA,key=U+0061\x1b\\");
    assert_eq!(ev(Press, "KeyC", c('c'), 4, None), "\x1bP=Fzke;t=p,code=KeyC,key=U+0063,mods=4\x1b\\");
    assert_eq!(ev(Press, "KeyA", c('ф'), 4, None), "\x1bP=Fzke;t=p,code=KeyA,key=U+0444,mods=4\x1b\\");
    assert_eq!(ev(Press, "Enter", n("Enter"), 0, Some("\r")), "\x1bP=Fzke;t=p,code=Enter,key=Enter,text=0d\x1b\\");
    assert_eq!(ev(Repeat, "ArrowUp", n("ArrowUp"), 0, None), "\x1bP=Fzke;t=r,code=ArrowUp,key=ArrowUp\x1b\\");
    assert_eq!(ev(Press, "ShiftLeft", n("Shift"), 1, None), "\x1bP=Fzke;t=p,code=ShiftLeft,key=Shift,mods=1\x1b\\");
    let k = KeyEvent { action: Press, code: "KeyA".into(), key: c('a'), mods: 0, locks: 1, text: Some("A".into()) };
    assert_eq!(encode_key(&k), "\x1bP=Fzke;t=p,code=KeyA,key=U+0061,locks=1,text=41\x1b\\");
    // Non-BMP text is hex UTF-8.
    assert!(ev(Press, "KeyA", c('a'), 0, Some("🦀")).contains("text=f09fa680"));
}

// README §3.2 multiplexer passthrough: tmux unwraps to the same message.
#[test]
fn tmux_unwrapped_message_is_identical() {
    let (mut t, out) = term();
    let inner = "\x1bP=Fzcq;id=1\x1b\\";
    // What tmux delivers after unwrapping is the inner message itself.
    t.feed(inner.as_bytes());
    let direct = out.take();
    t.feed(b"\x1bP=Fzcq;id=1\x1b\\");
    assert_eq!(out.take(), direct);
}
