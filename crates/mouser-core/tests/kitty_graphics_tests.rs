//! Kitty graphics protocol (fallback next to Fuzzball): transmission media,
//! formats, compression, placements, replies, deletes, relative placements,
//! Unicode placeholders, animation and the shared placement lifecycle.

use mouser_core::grid::{Cell, Color};
use mouser_core::kitty_graphics::{self, Cmd, PLACEHOLDER, diacritic};
use mouser_core::parser::TerminalParser;
use mouser_core::scrollback::{Scrollback, ScrollbackConfig};
use mouser_core::types::Size;
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
struct Out(Rc<RefCell<Vec<u8>>>);
impl Write for Out {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

type T = TerminalParser<Out>;

fn term() -> (T, Out) {
    let out = Out::default();
    let sb = Arc::new(Scrollback::new(ScrollbackConfig { max_scrollback_lines: 1000, chunk_lines: 2, ..Default::default() }));
    let mut t = TerminalParser::new(Size { cols: 20, rows: 6 }, sb, out.clone());
    t.cell_px = (10, 20);
    (t, out)
}

fn take(o: &Out) -> String {
    String::from_utf8(std::mem::take(&mut *o.0.borrow_mut())).unwrap()
}

fn b64(data: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
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

fn g(t: &mut T, keys: &str, payload: &[u8]) {
    t.feed(format!("\x1b_G{keys};{}\x1b\\", b64(payload)).as_bytes());
}

fn rgba(w: u32, h: u32, px: [u8; 4]) -> Vec<u8> {
    px.repeat((w * h) as usize)
}

fn kp(t: &T) -> Vec<(u32, u32)> {
    let mut v: Vec<_> = t.kitty_gfx.gfx.placements.iter().map(|p| (p.image_id, p.id)).collect();
    v.sort();
    v
}

#[test]
fn control_keys_parse() {
    let c = Cmd::parse(b"a=T,f=100,i=7,z=-5,q=2");
    assert_eq!(c.action(), b'T');
    assert_eq!((c.u(b'f'), c.u(b'i'), c.num(b'z'), c.num(b'q')), (100, 7, -5, 2));
    assert!(!c.bad);
    assert!(Cmd::parse(b"a=T,junk").bad);
    assert!(Cmd::parse(b"i=abc").bad);
}

#[test]
fn transmit_rgba_and_reply_ok() {
    let (mut t, o) = term();
    g(&mut t, "a=t,f=32,s=2,v=2,i=5", &rgba(2, 2, [1, 2, 3, 4]));
    assert_eq!(take(&o), "\x1b_Gi=5;OK\x1b\\");
    let im = &t.kitty_gfx.gfx.images[&5];
    assert_eq!((im.width, im.height, im.rgba[..4].to_vec()), (2, 2, vec![1, 2, 3, 4]));
    // Fuzzball's store is untouched: separate id namespace.
    assert!(t.fuzzball.gfx.images.is_empty());
}

#[test]
fn rgb_png_and_errors() {
    let (mut t, o) = term();
    g(&mut t, "f=24,s=1,v=1,i=1", &[9, 8, 7]);
    assert_eq!(t.kitty_gfx.gfx.images[&1].rgba.to_vec(), vec![9, 8, 7, 255]);
    take(&o);
    g(&mut t, "f=24,s=2,v=2,i=2", &[1, 2, 3]);
    assert!(take(&o).starts_with("\x1b_Gi=2;ENODATA:"));
    g(&mut t, "f=32,i=3", &[0; 4]);
    assert!(take(&o).starts_with("\x1b_Gi=3;EINVAL:"));
    g(&mut t, "f=100,i=4", b"not a png");
    assert!(take(&o).starts_with("\x1b_Gi=4;EBADPNG:"));
    let mut png = Vec::new();
    {
        let mut e = png::Encoder::new(&mut png, 1, 1);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(&[5, 6, 7, 8]).unwrap();
    }
    g(&mut t, "f=100,i=6", &png);
    assert_eq!(take(&o), "\x1b_Gi=6;OK\x1b\\");
    assert_eq!(t.kitty_gfx.gfx.images[&6].rgba.to_vec(), vec![5, 6, 7, 8]);
}

#[test]
fn indexed_formats_use_the_palette() {
    let (mut t, _o) = term();
    g(&mut t, "f=8,s=2,v=1,i=1", &[1, 249]);
    let px = t.kitty_gfx.gfx.images[&1].rgba.to_vec();
    assert_eq!(&px[..4], &[205, 0, 0, 255]);
    assert_eq!(&px[4..], &[220, 220, 220, 255]);
    g(&mut t, "f=16,s=1,v=1,i=2", &[2, 7]);
    assert_eq!(t.kitty_gfx.gfx.images[&2].rgba.to_vec(), vec![0, 205, 0, 7]);
}

#[test]
fn quiet_modes() {
    let (mut t, o) = term();
    g(&mut t, "f=24,s=1,v=1,i=1,q=1", &[0; 3]);
    assert_eq!(take(&o), "");
    g(&mut t, "f=24,s=2,v=2,i=1,q=1", &[0; 3]);
    assert!(take(&o).contains("ENODATA"));
    g(&mut t, "f=24,s=2,v=2,i=1,q=2", &[0; 3]);
    assert_eq!(take(&o), "");
    // No id: no reply at all.
    g(&mut t, "f=24,s=2,v=2", &[0; 3]);
    assert_eq!(take(&o), "");
}

#[test]
fn i_and_capital_i_together_is_einval() {
    let (mut t, o) = term();
    g(&mut t, "f=24,s=1,v=1,i=1,I=2", &[0; 3]);
    assert!(take(&o).contains(";EINVAL:"));
}

#[test]
fn chunked_direct_transmission() {
    let (mut t, o) = term();
    let data = b64(&rgba(4, 4, [7, 7, 7, 255]));
    let (a, b) = data.split_at(40);
    t.feed(format!("\x1b_Ga=T,f=32,s=4,v=4,i=9,m=1;{a}\x1b\\").as_bytes());
    assert!(t.kitty_gfx.upload_pending());
    assert!(t.kitty_gfx.gfx.images.is_empty());
    t.feed(format!("\x1b_Gm=0;{b}\x1b\\").as_bytes());
    assert!(!t.kitty_gfx.upload_pending());
    assert_eq!(t.kitty_gfx.gfx.images[&9].rgba.len(), 64);
    assert_eq!(kp(&t), vec![(9, 0)]);
    assert_eq!(take(&o), "\x1b_Gi=9;OK\x1b\\");
}

#[test]
fn delete_aborts_chunked_upload_and_upload_expires() {
    let (mut t, _o) = term();
    t.feed(b"\x1b_Ga=t,f=32,s=1,v=1,i=1,m=1;AAAA\x1b\\");
    t.feed(b"\x1b_Ga=d\x1b\\");
    assert!(!t.kitty_gfx.upload_pending());
    t.feed(b"\x1b_Ga=t,f=32,s=1,v=1,i=1,m=1;AAAA\x1b\\");
    assert_eq!(t.kitty_gfx.expire_upload(Instant::now() + Duration::from_secs(60)), None);
    assert!(!t.kitty_gfx.upload_pending());
}

#[test]
fn zlib_compression() {
    let (mut t, o) = term();
    let raw = rgba(3, 3, [1, 2, 3, 255]);
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(&raw).unwrap();
    g(&mut t, "f=32,s=3,v=3,o=z,i=2", &z.finish().unwrap());
    assert_eq!(take(&o), "\x1b_Gi=2;OK\x1b\\");
    assert_eq!(t.kitty_gfx.gfx.images[&2].rgba.to_vec(), raw);
    g(&mut t, "f=32,s=3,v=3,o=z,i=3", b"garbage");
    assert!(take(&o).contains(";EINVAL:"));
}

#[test]
fn file_media_size_offset_and_temp_deletion() {
    let (mut t, o) = term();
    let dir = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
    let f = dir.path().join("img.rgb");
    std::fs::write(&f, [0, 0, 9, 8, 7, 0]).unwrap();
    g(&mut t, "t=f,f=24,s=1,v=1,S=3,O=2,i=1", f.to_str().unwrap().as_bytes());
    assert_eq!(take(&o), "\x1b_Gi=1;OK\x1b\\");
    assert_eq!(t.kitty_gfx.gfx.images[&1].rgba.to_vec(), vec![9, 8, 7, 255]);
    assert!(f.exists(), "t=f never deletes");
    // Too short for S: the one generic error.
    g(&mut t, "t=f,f=24,s=1,v=1,S=30,i=2", f.to_str().unwrap().as_bytes());
    assert_eq!(take(&o), "\x1b_Gi=2;EBADF:Failed to read image file\x1b\\");
    g(&mut t, "t=f,f=24,s=1,v=1,i=2", b"/nonexistent/file");
    assert_eq!(take(&o), "\x1b_Gi=2;EBADF:Failed to read image file\x1b\\");
    g(&mut t, "t=f,f=24,s=1,v=1,i=2", dir.path().to_str().unwrap().as_bytes());
    assert_eq!(take(&o), "\x1b_Gi=2;EBADF:Failed to read image file\x1b\\");
    g(&mut t, "t=f,f=24,s=1,v=1,i=2", b"/dev/zero");
    assert_eq!(take(&o), "\x1b_Gi=2;EBADF:Failed to read image file\x1b\\");
    // Temp file: deleted only with the marker in its path.
    let keep = dir.path().join("plain.rgb");
    std::fs::write(&keep, [1, 2, 3]).unwrap();
    g(&mut t, "t=t,f=24,s=1,v=1,i=3", keep.to_str().unwrap().as_bytes());
    assert!(keep.exists());
    let gone = dir.path().join("x-tty-graphics-protocol-1.rgb");
    std::fs::write(&gone, [1, 2, 3]).unwrap();
    g(&mut t, "t=t,f=24,s=1,v=1,i=4", gone.to_str().unwrap().as_bytes());
    assert!(!gone.exists());
    assert!(t.kitty_gfx.gfx.images.contains_key(&4));
}

#[cfg(target_os = "linux")]
#[test]
fn shared_memory_media() {
    let (mut t, o) = term();
    let name = format!("/mouser-test-{}", std::process::id());
    let c = std::ffi::CString::new(name.clone()).unwrap();
    unsafe {
        let fd = libc::shm_open(c.as_ptr(), libc::O_CREAT | libc::O_RDWR, 0o600);
        assert!(fd >= 0);
        assert_eq!(libc::write(fd, [4u8, 5, 6].as_ptr() as *const _, 3), 3);
        libc::close(fd);
    }
    g(&mut t, "t=s,f=24,s=1,v=1,i=1", name.as_bytes());
    assert_eq!(take(&o), "\x1b_Gi=1;OK\x1b\\");
    assert_eq!(t.kitty_gfx.gfx.images[&1].rgba.to_vec(), vec![4, 5, 6, 255]);
    // Unlinked after reading.
    assert!(unsafe { libc::shm_open(c.as_ptr(), libc::O_RDONLY, 0) } < 0);
    g(&mut t, "t=s,f=24,s=1,v=1,i=2", name.as_bytes());
    assert!(take(&o).contains("EBADF"));
}

#[test]
fn query_action_does_not_store() {
    let (mut t, o) = term();
    t.feed(b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\");
    assert_eq!(take(&o), "\x1b_Gi=31;OK\x1b\\");
    assert!(t.kitty_gfx.gfx.images.is_empty());
}

#[test]
fn feature_query() {
    let (mut t, o) = term();
    t.feed(b"\x1b_Ga=q,i=31,e=T,U=1,C=1,f=100,o=z\x1b\\");
    let r = take(&o);
    assert!(r.starts_with("\x1b_Gi=31,e=T"), "{r}");
    for k in ["U=1", "C=1", "f=100", "o=z"] {
        assert!(r.contains(k), "{r} lacks {k}");
    }
    t.feed(b"\x1b_Ga=q,i=32,e=f,r=1,X=1\x1b\\");
    let r = take(&o);
    assert!(r.contains("e=f") && r.contains("r=1") && r.contains("X=1"), "{r}");
}

#[test]
fn image_numbers_get_terminal_ids() {
    let (mut t, o) = term();
    g(&mut t, "f=24,s=1,v=1,I=13", &[0; 3]);
    let r = take(&o);
    let id: u32 = r.trim_start_matches("\x1b_Gi=").split(',').next().unwrap().parse().unwrap();
    assert_eq!(r, format!("\x1b_Gi={id},I=13;OK\x1b\\"));
    t.feed(b"\x1b_Ga=p,I=13,p=2\x1b\\");
    assert_eq!(take(&o), format!("\x1b_Gi={id},I=13,p=2;OK\x1b\\"));
    assert_eq!(kp(&t), vec![(id, 2)]);
    t.feed(b"\x1b_Ga=d,d=N,I=13\x1b\\");
    assert!(t.kitty_gfx.gfx.images.is_empty());
}

#[test]
fn put_missing_image_is_enoent() {
    let (mut t, o) = term();
    t.feed(b"\x1b_Ga=p,i=77\x1b\\");
    assert!(take(&o).starts_with("\x1b_Gi=77;ENOENT:"));
}

#[test]
fn placement_size_offsets_and_cursor() {
    let (mut t, _o) = term();
    // 25x30 px at cell 10x20 with X=6: 4 cols (31 px), 2 rows; cursor moves.
    t.feed(b"\x1b[2;3H");
    g(&mut t, "a=T,f=32,s=25,v=30,i=1,X=6,Y=3", &rgba(25, 30, [0; 4]));
    let p = &t.kitty_gfx.gfx.placements[0];
    assert_eq!((p.col, p.cols, p.rows, p.row - t.lines_scrolled), (2, 4, 2, 1));
    assert_eq!(p.draw_rect((10, 20)), (6, 3, 25, 30));
    assert_eq!((t.screen.cursor.x, t.screen.cursor.y), (6, 2));
    // C=1 keeps the cursor.
    g(&mut t, "a=T,f=32,s=1,v=1,i=2,C=1", &[0; 4]);
    assert_eq!((t.screen.cursor.x, t.screen.cursor.y), (6, 2));
}

#[test]
fn columns_rows_and_source_rect() {
    let (mut t, _o) = term();
    g(&mut t, "f=32,s=40,v=20,i=1", &rgba(40, 20, [0; 4]));
    t.feed(b"\x1b_Ga=p,i=1,p=1,c=4,C=1\x1b\\");
    let p = t.kitty_gfx.gfx.placements.iter().find(|p| p.id == 1).unwrap().clone();
    // 4 cols = 40 px wide, aspect 2:1 -> 20 px = 1 row.
    assert_eq!((p.cols, p.rows), (4, 1));
    t.feed(b"\x1b_Ga=p,i=1,p=2,c=4,r=4,C=1\x1b\\");
    let p = t.kitty_gfx.gfx.placements.iter().find(|p| p.id == 2).unwrap().clone();
    // Letterboxed: 40x80 box, 40x20 image centred vertically.
    assert_eq!(p.draw_rect((10, 20)), (0, 30, 40, 20));
    t.feed(b"\x1b_Ga=p,i=1,p=3,x=10,y=5,w=100,h=5,C=1\x1b\\");
    let p = t.kitty_gfx.gfx.placements.iter().find(|p| p.id == 3).unwrap().clone();
    assert_eq!(p.src, (10, 5, 30, 5));
}

#[test]
fn same_ids_replace_unnamed_placements_accumulate() {
    let (mut t, _o) = term();
    g(&mut t, "f=32,s=1,v=1,i=1", &[0; 4]);
    t.feed(b"\x1b_Ga=p,i=1,C=1\x1b\\\x1b_Ga=p,i=1,C=1\x1b\\");
    assert_eq!(kp(&t), vec![(1, 0), (1, 0)]);
    t.feed(b"\x1b_Ga=p,i=1,p=5,C=1\x1b\\\x1b[3;3H\x1b_Ga=p,i=1,p=5,C=1\x1b\\");
    assert_eq!(kp(&t), vec![(1, 0), (1, 0), (1, 5)]);
    // Retransmitting drops the image's placements.
    g(&mut t, "f=32,s=1,v=1,i=1", &[0; 4]);
    assert!(kp(&t).is_empty());
}

#[test]
fn anonymous_images_never_reply() {
    let (mut t, o) = term();
    g(&mut t, "a=T,f=32,s=1,v=1", &[0; 4]);
    g(&mut t, "a=T,f=32,s=1,v=1", &[0; 4]);
    assert_eq!(take(&o), "");
    assert_eq!(t.kitty_gfx.gfx.images.len(), 2);
}

fn three(t: &mut T) {
    g(t, "f=32,s=10,v=20,i=1", &rgba(10, 20, [0; 4]));
    g(t, "f=32,s=10,v=20,i=2", &rgba(10, 20, [0; 4]));
    // (1,1) at row 1 col 1, (1,2) at row 3 col 5 z=4, (2,1) at row 1 col 9.
    t.feed(b"\x1b[1;1H\x1b_Ga=p,i=1,p=1,C=1\x1b\\");
    t.feed(b"\x1b[3;5H\x1b_Ga=p,i=1,p=2,z=4,C=1\x1b\\");
    t.feed(b"\x1b[1;9H\x1b_Ga=p,i=2,p=1,C=1\x1b\\");
}

#[test]
fn delete_specifiers() {
    let cases: &[(&str, &[(u32, u32)])] = &[
        ("d=a", &[]),
        ("d=i,i=1", &[(2, 1)]),
        ("d=i,i=1,p=2", &[(1, 1), (2, 1)]),
        ("d=p,x=5,y=3", &[(1, 1), (2, 1)]),
        ("d=q,x=5,y=3,z=0", &[(1, 1), (1, 2), (2, 1)]),
        ("d=q,x=5,y=3,z=4", &[(1, 1), (2, 1)]),
        ("d=x,x=9", &[(1, 1), (1, 2)]),
        ("d=y,y=1", &[(1, 2)]),
        ("d=z,z=4", &[(1, 1), (2, 1)]),
        ("d=r,x=2,y=5", &[(1, 1), (1, 2)]),
    ];
    for (keys, left) in cases {
        let (mut t, _o) = term();
        three(&mut t);
        t.feed(format!("\x1b_Ga=d,{keys}\x1b\\").as_bytes());
        assert_eq!(kp(&t), left.to_vec(), "{keys}");
        assert_eq!(t.kitty_gfx.gfx.images.len(), 2, "{keys}: lowercase keeps data");
    }
    let (mut t, _o) = term();
    three(&mut t);
    t.feed(b"\x1b[1;1H\x1b_Ga=d,d=c\x1b\\");
    assert_eq!(kp(&t), vec![(1, 2), (2, 1)]);
}

#[test]
fn uppercase_deletes_free_data() {
    let (mut t, _o) = term();
    three(&mut t);
    t.feed(b"\x1b_Ga=d,d=Y,y=1\x1b\\");
    // Image 2 lost its only placement; image 1 still has (1,2).
    assert!(t.kitty_gfx.gfx.images.contains_key(&1));
    assert!(!t.kitty_gfx.gfx.images.contains_key(&2));
    t.feed(b"\x1b_Ga=d,d=I,i=1\x1b\\");
    assert!(t.kitty_gfx.gfx.images.is_empty());
    let (mut t, _o) = term();
    three(&mut t);
    t.feed(b"\x1b_Ga=d,d=A\x1b\\");
    assert!(t.kitty_gfx.gfx.images.is_empty());
    let (mut t, _o) = term();
    three(&mut t);
    t.feed(b"\x1b_Ga=d,d=R,x=1,y=1\x1b\\");
    assert_eq!(t.kitty_gfx.gfx.images.keys().copied().collect::<Vec<_>>(), vec![2]);
}

#[test]
fn relative_placements() {
    let (mut t, o) = term();
    three(&mut t);
    take(&o);
    t.feed(b"\x1b_Ga=p,i=2,p=7,P=1,Q=2,H=2,V=-1\x1b\\");
    assert_eq!(take(&o), "\x1b_Gi=2,p=7;OK\x1b\\");
    let c = t.kitty_gfx.gfx.placements.iter().find(|p| (p.image_id, p.id) == (2, 7)).unwrap();
    assert_eq!((c.row - t.lines_scrolled, c.col), (1, 6));
    // Moving the parent moves the child.
    t.feed(b"\x1b[5;1H\x1b_Ga=p,i=1,p=2,z=4,C=1\x1b\\");
    let c = t.kitty_gfx.gfx.placements.iter().find(|p| (p.image_id, p.id) == (2, 7)).unwrap();
    assert_eq!((c.row - t.lines_scrolled, c.col), (3, 2));
    t.feed(b"\x1b_Ga=p,i=2,p=8,P=9,Q=9\x1b\\");
    assert!(take(&o).contains("ENOPARENT"));
    t.feed(b"\x1b_Ga=p,i=1,p=2,P=2,Q=7\x1b\\");
    assert!(take(&o).contains("ECYCLE"));
    t.feed(b"\x1b_Ga=p,i=1,p=9,U=1,P=2,Q=7\x1b\\");
    assert!(take(&o).contains("EINVAL"));
    // Deleting the parent deletes the child.
    t.feed(b"\x1b_Ga=d,d=i,i=1,p=2\x1b\\");
    assert!(!kp(&t).contains(&(2, 7)));
}

#[test]
fn relative_chain_too_deep() {
    let (mut t, o) = term();
    g(&mut t, "f=32,s=1,v=1,i=1", &[0; 4]);
    t.feed(b"\x1b_Ga=p,i=1,p=1,C=1\x1b\\");
    // A chain of 9 (8 parents above the last) is allowed, one more is not.
    for p in 2..=9 {
        t.feed(format!("\x1b_Ga=p,i=1,p={p},P=1,Q={}\x1b\\", p - 1).as_bytes());
    }
    take(&o);
    assert!(!take(&o).contains('E'));
    t.feed(b"\x1b_Ga=p,i=1,p=10,P=1,Q=9\x1b\\");
    assert!(take(&o).contains("ETOODEEP"));
}

#[test]
fn lifecycle_scroll_scrollback_clear_and_alt() {
    let (mut t, _o) = term();
    g(&mut t, "f=32,s=10,v=20,i=1", &rgba(10, 20, [0; 4]));
    t.feed(b"\x1b[2;1H\x1b_Ga=p,i=1,p=1,C=1\x1b\\");
    // Erase line / chars / ED 0 leave Kitty placements alone.
    t.feed(b"\x1b[2;1H\x1b[2K\x1b[5X\x1b[1;1H\x1b[J");
    assert_eq!(kp(&t), vec![(1, 1)]);
    // Scrolls with the text into the scrollback and stays there.
    t.feed(b"\x1b[6;1H\n\n\n");
    let p = &t.kitty_gfx.gfx.placements[0];
    assert_eq!(p.row - t.lines_scrolled, -2);
    // ED 3 drops what only the scrollback held.
    t.feed(b"\x1b[3J");
    assert!(kp(&t).is_empty());
    // ED 2 clears visible Kitty placements.
    t.feed(b"\x1b[1;1H\x1b_Ga=p,i=1,p=2,C=1\x1b\\\x1b[2J");
    assert!(kp(&t).is_empty());
    // Alt screen placements go when leaving it.
    t.feed(b"\x1b[?1049h\x1b_Ga=p,i=1,p=3,C=1\x1b\\");
    assert_eq!(kp(&t), vec![(1, 3)]);
    t.feed(b"\x1b[?1049l");
    assert!(kp(&t).is_empty());
    // Reset clears everything.
    g(&mut t, "a=T,f=32,s=1,v=1,i=4", &[0; 4]);
    t.feed(b"\x1bc");
    assert!(t.kitty_gfx.gfx.images.is_empty());
}

#[test]
fn only_placements_inside_the_region_scroll() {
    let (mut t, _o) = term();
    g(&mut t, "f=32,s=10,v=40,i=1", &rgba(10, 40, [0; 4]));
    // Two rows tall at rows 2-3; region rows 3-6 does not contain it.
    t.feed(b"\x1b[2;1H\x1b_Ga=p,i=1,p=1,C=1\x1b\\");
    t.feed(b"\x1b[4;1H\x1b_Ga=p,i=1,p=2,C=1\x1b\\");
    t.feed(b"\x1b[3;6r\x1b[6;1H\n");
    let row = |t: &T, pid| t.kitty_gfx.gfx.placements.iter().find(|p| p.id == pid).map(|p| p.row - t.lines_scrolled);
    assert_eq!(row(&t, 1), Some(1));
    assert_eq!(row(&t, 2), Some(2));
}

#[test]
fn resize_scales_placements_that_fitted() {
    let (mut t, _o) = term();
    g(&mut t, "f=32,s=40,v=20,i=1", &rgba(40, 20, [0; 4]));
    t.feed(b"\x1b[1;1H\x1b_Ga=p,i=1,p=1,c=8,C=1\x1b\\");
    t.resize(Size { cols: 4, rows: 6 });
    let p = &t.kitty_gfx.gfx.placements[0];
    assert_eq!(p.cols, 4);
    t.resize(Size { cols: 20, rows: 6 });
    assert_eq!(t.kitty_gfx.gfx.placements[0].cols, 8);
}

#[test]
fn shared_quota_and_precedence_of_namespaces() {
    let (mut t, _o) = term();
    t.feed(b"\x1bP=Fzgu;id=1,fmt=rgba,w=1,h=1,size=4,len=8:AAAAAA==\x1b\\");
    g(&mut t, "f=32,s=1,v=1,i=1", &[0; 4]);
    // Same id in both protocols: two separate images.
    assert_eq!(t.fuzzball.gfx.images.len(), 1);
    assert_eq!(t.kitty_gfx.gfx.images.len(), 1);
    assert_eq!(t.kitty_gfx.mem_used() + t.fuzzball.gfx.mem_used(), 8);
}

// -- Unicode placeholders ---------------------------------------------------

fn ph(fg: Color, ul: Color, d: &[u32]) -> Cell {
    let mut c = Cell { ch: PLACEHOLDER, ..Cell::default() };
    c.style.fg = fg;
    c.style.ul_color = ul;
    for (i, n) in d.iter().enumerate() {
        c.comb[i] = diacritic(*n).unwrap();
    }
    c
}

#[test]
fn diacritic_table() {
    assert_eq!(diacritic(0), Some('\u{305}'));
    assert_eq!(diacritic(1), Some('\u{30D}'));
    assert_eq!(kitty_graphics::diacritic_value('\u{30E}'), Some(2));
    assert_eq!(kitty_graphics::diacritic_value('a'), None);
}

#[test]
fn placeholders_draw_the_virtual_placement() {
    let (mut t, o) = term();
    g(&mut t, "a=T,U=1,f=32,s=20,v=40,i=42,c=2,r=2,q=2", &rgba(20, 40, [0; 4]));
    assert_eq!(take(&o), "");
    assert_eq!(t.screen.cursor.x, 0, "virtual placements do not move the cursor");
    assert!(t.kitty_gfx.gfx.draws(false, t.lines_scrolled, 6, (10, 20)).is_empty());
    // Via printing: 2x2 placeholder, id in a 256-colour fg.
    t.feed("\x1b[38;5;42m\u{10EEEE}\u{305}\u{305}\u{10EEEE}\u{305}\u{30D}\r\n\u{10EEEE}\u{30D}\u{305}\u{10EEEE}\u{30D}\u{30D}\x1b[39m".as_bytes());
    let screen = &t.screen;
    let row = |i: usize| (i < 6).then(|| screen.row(i));
    let d = t.kitty_gfx.placeholder_draws(6, &row, (10, 20));
    // Image 20x40 into a 20x40 box: one draw per row run, each a 20x20 slice.
    assert_eq!(d.len(), 2);
    assert_eq!((d[0].row, d[0].col, d[0].width, d[0].height, d[0].src), (0, 0, 20, 20, (0, 0, 20, 20)));
    assert_eq!((d[1].row, d[1].src), (1, (0, 20, 20, 20)));
}

#[test]
fn placeholder_inference_and_id_bytes() {
    let (mut t, _o) = term();
    let id = 42 + (2 << 24);
    g(&mut t, &format!("a=T,U=1,f=32,s=30,v=20,i={id},p=3,c=3,r=1,q=2"), &rgba(30, 20, [0; 4]));
    // Only the first cell has diacritics (row, col, msb); the rest infer.
    let fg = Color::Rgb(0, 0, 42);
    let cells = [ph(fg, Color::Indexed(3), &[0, 0, 2]), ph(fg, Color::Indexed(3), &[]), ph(fg, Color::Indexed(3), &[])];
    let row = |i: usize| (i == 0).then_some(&cells[..]);
    let d = t.kitty_gfx.placeholder_draws(1, &row, (10, 20));
    assert_eq!(d.len(), 1);
    assert_eq!((d[0].image_id, d[0].width, d[0].height), (id, 30, 20));
    // Wrong placement id in the underline colour: nothing.
    let cells = [ph(fg, Color::Indexed(4), &[0, 0, 2])];
    let row = |i: usize| (i == 0).then_some(&cells[..]);
    assert!(t.kitty_gfx.placeholder_draws(1, &row, (10, 20)).is_empty());
}

#[test]
fn virtual_placements_survive_screen_deletes() {
    let (mut t, _o) = term();
    g(&mut t, "a=T,U=1,f=32,s=1,v=1,i=5,c=1,r=1,q=2", &[0; 4]);
    for k in ["d=a", "d=c", "d=p,x=1,y=1", "d=x,x=1", "d=y,y=1", "d=z,z=0"] {
        t.feed(format!("\x1b_Ga=d,{k}\x1b\\").as_bytes());
        assert_eq!(kp(&t).len(), 1, "{k}");
    }
    t.feed(b"\x1b[2J\x1b[?1049h\x1b[?1049l");
    assert_eq!(kp(&t).len(), 1);
    t.feed(b"\x1b_Ga=d,d=i,i=5\x1b\\");
    assert!(kp(&t).is_empty());
}

#[test]
fn relative_to_a_virtual_placement() {
    let (mut t, _o) = term();
    g(&mut t, "a=T,U=1,f=32,s=10,v=20,i=5,p=1,c=1,r=1,q=2", &rgba(10, 20, [0; 4]));
    g(&mut t, "f=32,s=10,v=20,i=6", &rgba(10, 20, [0; 4]));
    t.feed(b"\x1b_Ga=p,i=6,p=1,P=5,Q=1,H=1,V=1,q=2\x1b\\");
    let cells = [Cell::default(), Cell::default(), ph(Color::Indexed(5), Color::Indexed(1), &[0, 0])];
    let row = |i: usize| (i == 2).then_some(&cells[..]);
    let d = t.kitty_gfx.placeholder_draws(6, &row, (10, 20));
    let child = d.iter().find(|d| d.image_id == 6).expect("child drawn");
    assert_eq!((child.row, child.col), (3, 3));
}

// -- Animation --------------------------------------------------------------

#[test]
fn animation_frames_and_control() {
    let (mut t, o) = term();
    g(&mut t, "a=T,f=32,s=1,v=1,i=1,C=1", &[1, 0, 0, 255]);
    g(&mut t, "a=f,i=1,f=32,s=1,v=1,z=100", &[2, 0, 0, 255]);
    assert_eq!(take(&o), "\x1b_Gi=1;OK\x1b\\\x1b_Gi=1;OK\x1b\\");
    // Frame 3 based on frame 2, edited in place.
    t.feed(b"\x1b_Ga=f,i=1,c=2,z=100;\x1b\\");
    g(&mut t, "a=f,i=1,r=3,f=32,s=1,v=1,X=1", &[3, 0, 0, 255]);
    assert_eq!(t.kitty_gfx.current_frame(1), Some((0, 3)));
    // Client-driven: show frame 3.
    t.feed(b"\x1b_Ga=a,i=1,c=3\x1b\\");
    assert_eq!(t.kitty_gfx.gfx.images[&1].rgba[0], 3);
    // Terminal-driven, looping: root gap 100 too.
    t.feed(b"\x1b_Ga=a,i=1,r=1,z=100\x1b\\\x1b_Ga=a,i=1,c=1,s=3\x1b\\");
    let t0 = Instant::now();
    let (changed, next) = t.kitty_gfx.tick(t0 + Duration::from_millis(150));
    assert!(changed);
    assert!(next.is_some());
    assert_eq!(t.kitty_gfx.gfx.images[&1].rgba[0], 2);
    t.kitty_gfx.tick(t0 + Duration::from_millis(250));
    assert_eq!(t.kitty_gfx.gfx.images[&1].rgba[0], 3);
    t.kitty_gfx.tick(t0 + Duration::from_millis(350));
    assert_eq!(t.kitty_gfx.gfx.images[&1].rgba[0], 1, "loops back");
    t.feed(b"\x1b_Ga=a,i=1,s=1\x1b\\");
    assert_eq!(t.kitty_gfx.next_tick(), None);
    // Delete frames: back to the root frame.
    t.feed(b"\x1b_Ga=d,d=f,i=1\x1b\\");
    assert_eq!(t.kitty_gfx.current_frame(1), Some((0, 1)));
}

#[test]
fn animation_loop_count_and_loading_mode() {
    let (mut t, _o) = term();
    g(&mut t, "a=t,f=32,s=1,v=1,i=1", &[1, 0, 0, 255]);
    g(&mut t, "a=f,i=1,f=32,s=1,v=1,z=10", &[2, 0, 0, 255]);
    // v=2: loop once, then stop.
    t.feed(b"\x1b_Ga=a,i=1,r=1,z=10\x1b\\\x1b_Ga=a,i=1,s=3,v=2\x1b\\");
    let t0 = Instant::now();
    for ms in [15, 25, 35, 45, 55, 65] {
        t.kitty_gfx.tick(t0 + Duration::from_millis(ms));
    }
    assert_eq!(t.kitty_gfx.next_tick(), None, "stopped after the loops");
    // s=2 waits at the last frame for more.
    t.feed(b"\x1b_Ga=a,i=1,c=1,s=2\x1b\\");
    t.kitty_gfx.tick(Instant::now() + Duration::from_millis(500));
    assert_eq!(t.kitty_gfx.current_frame(1), Some((1, 2)));
    assert_eq!(t.kitty_gfx.next_tick(), None);
    g(&mut t, "a=f,i=1,f=32,s=1,v=1,z=10", &[3, 0, 0, 255]);
    assert!(t.kitty_gfx.next_tick().is_some(), "resumes when a frame arrives");
}

#[test]
fn compose_frames() {
    let (mut t, o) = term();
    g(&mut t, "a=t,f=32,s=2,v=1,i=1", &[1, 0, 0, 255, 2, 0, 0, 255]);
    t.feed(b"\x1b_Ga=f,i=1,Y=4278190335;\x1b\\");
    take(&o);
    // Copy root's right pixel onto frame 2's left pixel.
    t.feed(b"\x1b_Ga=c,i=1,r=1,c=2,w=1,h=1,X=1,Y=0,x=0,y=0,C=1\x1b\\");
    assert_eq!(take(&o), "");
    t.feed(b"\x1b_Ga=a,i=1,c=2\x1b\\");
    assert_eq!(t.kitty_gfx.gfx.images[&1].rgba.to_vec(), vec![2, 0, 0, 255, 255, 0, 0, 255]);
    t.feed(b"\x1b_Ga=c,i=1,r=1,c=2,w=3\x1b\\");
    assert!(take(&o).contains("EINVAL"));
    t.feed(b"\x1b_Ga=c,i=1,r=1,c=1\x1b\\");
    assert!(take(&o).contains("EINVAL"));
    t.feed(b"\x1b_Ga=c,i=1,r=5,c=1\x1b\\");
    assert!(take(&o).contains("ENOENT"));
}

#[test]
fn parser_graphics_timers_drive_animation() {
    let (mut t, _o) = term();
    assert!(!t.graphics_timers_pending());
    g(&mut t, "a=T,f=32,s=1,v=1,i=1,C=1", &[1, 0, 0, 255]);
    g(&mut t, "a=f,i=1,f=32,s=1,v=1,z=20", &[2, 0, 0, 255]);
    t.feed(b"\x1b_Ga=a,i=1,r=1,z=20\x1b\\\x1b_Ga=a,i=1,s=3\x1b\\");
    assert!(t.graphics_timers_pending());
    let gen0 = t.graphics_generation;
    let next = t.graphics_timers(Instant::now() + Duration::from_millis(30));
    assert!(next.is_some());
    assert!(t.graphics_generation > gen0, "a frame change bumps the generation");
    t.feed(b"\x1b_Ga=a,i=1,s=1\x1b\\");
    assert!(!t.graphics_timers_pending());
}
