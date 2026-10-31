//! Several images on screen at once: after every resize no two placements on the
//! main screen and in the scrollback overlap, and no placement covers text.

use mouser_core::graphics::Placement;
use mouser_core::parser::TerminalParser;
use mouser_core::scrollback::{Scrollback, ScrollbackConfig};
use mouser_core::types::Size;
use std::sync::Arc;

type T = TerminalParser<std::io::Sink>;

fn term(cols: usize, rows: usize) -> T {
    let cfg = ScrollbackConfig { chunk_lines: 8, ..Default::default() };
    let mut t = TerminalParser::new(Size { cols, rows }, Arc::new(Scrollback::new(cfg)), std::io::sink());
    t.cell_px = (10, 20);
    // Fuzzball images 1: 2x2 px, 2: 8x2 (wide), 3: 2x8 (tall), all RGBA.
    for (id, w, h) in [(1, 2, 2), (2, 8, 2), (3, 2, 8)] {
        let px = vec![200u8; w * h * 4];
        let b = b64(&px);
        t.feed(format!("\x1bP=Fzgu;id={id},fmt=rgba,w={w},h={h},size={},len={}:{b}\x1b\\", px.len(), b.len()).as_bytes());
    }
    t
}

fn b64(d: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in d.chunks(3) {
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

/// As `mouser show` (default): new line if needed, fill the width, continue below.
fn show_fit(t: &mut T, id: u32, pid: u32) {
    if t.screen.cursor.x > 0 {
        t.feed(b"\r\n");
    }
    t.feed(format!("\x1bP=Fzgp;id={id},pid={pid},cols=1,fit=w,quiet=1\x1b\\\r\n").as_bytes());
}

/// As `mouser show --width N`: fixed size at the cursor, continue below.
fn show_fixed(t: &mut T, id: u32, pid: u32, cols: u32) {
    t.feed(format!("\x1bP=Fzgp;id={id},pid={pid},cols={cols},quiet=1\x1b\\\r\n").as_bytes());
}

/// Kitty `a=T` with explicit `c`/`r` (cursor moves below), like `icat`.
fn kitty(t: &mut T, id: u32, c: u32, r: u32) {
    t.feed(format!("\x1b_Ga=T,f=32,s=2,v=2,i={id},c={c},r={r},q=2;{}\x1b\\\r\n", b64(&[90u8; 16])).as_bytes());
}

fn placements(t: &T) -> Vec<(&'static str, Placement)> {
    let mut v: Vec<(&'static str, Placement)> = t.fuzzball.gfx.placements.iter().map(|p| ("fz", p.clone())).collect();
    v.extend(t.kitty_gfx.gfx.placements.iter().map(|p| ("kitty", p.clone())));
    v.retain(|(_, p)| !p.alt && !p.virt);
    v
}

/// Text of absolute row `abs` (scrollback or screen), one char per column.
fn row_text(t: &T, abs: i64) -> Option<String> {
    let first = t.lines_scrolled - t.scrollback.total_lines() as i64;
    if abs < first {
        return None;
    }
    if abs < t.lines_scrolled {
        return t.scrollback.get_lines((abs - first) as usize, 1).pop();
    }
    let y = (abs - t.lines_scrolled) as usize;
    (y < t.screen.size.rows).then(|| t.screen.row_string(y))
}

/// No two placements share a cell; no placement covers text.
fn check(t: &T, step: &str) {
    let ps = placements(t);
    for (i, (ka, a)) in ps.iter().enumerate() {
        for (kb, b) in &ps[i + 1..] {
            let rows = a.row < b.row + b.rows as i64 && b.row < a.row + a.rows as i64;
            let cols = a.col < b.col + b.cols && b.col < a.col + a.cols;
            assert!(
                !(rows && cols),
                "{step}: {ka} {}/{} at ({},{}) {}x{} overlaps {kb} {}/{} at ({},{}) {}x{}",
                a.image_id,
                a.id,
                a.row,
                a.col,
                a.cols,
                a.rows,
                b.image_id,
                b.id,
                b.row,
                b.col,
                b.cols,
                b.rows
            );
        }
        for r in a.row..a.row + a.rows as i64 {
            if let Some(s) = row_text(t, r) {
                let covered: String = s.chars().skip(a.col as usize).take(a.cols as usize).collect();
                assert!(covered.trim().is_empty(), "{step}: {ka} {}/{} covers text {covered:?} on row {r}", a.image_id, a.id);
            }
        }
    }
}

const STEPS: &[(usize, usize)] = &[(40, 30), (80, 30), (13, 30), (57, 12), (7, 30), (120, 50), (25, 8), (40, 30), (90, 30), (40, 30)];

fn run(t: &mut T, name: &str) {
    let n = placements(t).len();
    assert!(n >= 3, "{name}: placements were made");
    check(t, &format!("{name} initial"));
    for &(c, r) in STEPS {
        t.resize(Size { cols: c, rows: r });
        check(t, &format!("{name} {c}x{r}"));
        assert_eq!(placements(t).len(), n, "{name} {c}x{r}: no placement dropped");
    }
}

const LONG: &str = "Some text that is long enough to wrap at most widths used here, really.";

#[test]
fn consecutive_fitted_images_one_show() {
    let mut t = term(40, 30);
    t.feed(b"$ mouser show a.png b.png c.png\r\n");
    for (i, id) in [1, 2, 3].into_iter().enumerate() {
        show_fit(&mut t, id, i as u32 + 1);
    }
    t.feed(b"$ ");
    run(&mut t, "one show");
}

#[test]
fn separate_shows_back_to_back() {
    let mut t = term(40, 30);
    for id in [2, 1, 2, 3] {
        t.feed(b"$ mouser show x.png\r\n");
        show_fit(&mut t, id, 1);
    }
    t.feed(b"$ ");
    run(&mut t, "separate shows");
}

#[test]
fn images_interleaved_with_wrapped_text() {
    let mut t = term(40, 30);
    for id in [1, 3, 2] {
        t.feed(format!("{LONG}\r\n").as_bytes());
        show_fit(&mut t, id, id);
        t.feed(b"short line\r\n");
    }
    t.feed(LONG.as_bytes());
    run(&mut t, "interleaved");
}

#[test]
fn fixed_size_images_next_to_fitted_ones() {
    let mut t = term(40, 30);
    show_fit(&mut t, 2, 1);
    show_fixed(&mut t, 1, 2, 6);
    t.feed(b"text\r\n");
    show_fixed(&mut t, 3, 3, 30);
    show_fit(&mut t, 1, 4);
    // --original: natural size (2x2 px = 1x1 cell).
    t.feed(b"\x1bP=Fzgp;id=1,pid=5,quiet=1\x1b\\\r\n");
    show_fit(&mut t, 3, 6);
    t.feed(b"$ ");
    run(&mut t, "fixed and fitted");
}

#[test]
fn kitty_explicit_size_next_to_fuzzball() {
    let mut t = term(40, 30);
    show_fit(&mut t, 2, 1);
    kitty(&mut t, 50, 8, 3);
    show_fit(&mut t, 1, 2);
    kitty(&mut t, 51, 30, 2);
    t.feed(format!("{LONG}\r\n").as_bytes());
    kitty(&mut t, 52, 4, 4);
    show_fit(&mut t, 3, 3);
    t.feed(b"$ ");
    run(&mut t, "kitty and fuzzball");
}

#[test]
fn images_partly_in_the_scrollback() {
    let mut t = term(40, 12);
    for id in [2, 1, 3, 2, 2] {
        t.feed(format!("{LONG}\r\n").as_bytes());
        let pid = 1 + t.fuzzball.gfx.placements.len() as u32;
        show_fit(&mut t, id, pid);
    }
    t.feed(b"$ ");
    assert!(placements(&t).iter().any(|(_, p)| p.row < t.lines_scrolled), "some anchors in the scrollback");
    run(&mut t, "scrollback");
}

#[test]
fn many_small_images_and_text() {
    let mut t = term(40, 30);
    for i in 0..12u32 {
        if i % 3 == 0 {
            t.feed(format!("{i}: {LONG}\r\n").as_bytes());
        }
        if i % 2 == 0 {
            show_fit(&mut t, 1 + i % 3, i + 1);
        } else {
            show_fixed(&mut t, 1 + i % 3, i + 1, 3 + i);
        }
    }
    t.feed(b"$ ");
    run(&mut t, "many");
}

/// Placements an app put on top of each other on purpose stay that way (allowed).
#[test]
fn explicit_overlap_is_left_alone() {
    let mut t = term(40, 30);
    t.feed(b"\x1bP=Fzgp;id=1,pid=1,cols=10,rows=5,move=0\x1b\\\x1b[2;3H\x1bP=Fzgp;id=2,pid=2,cols=8,rows=2,z=1,move=0\x1b\\");
    let n = t.fuzzball.gfx.placements.len();
    for &(c, r) in STEPS {
        t.resize(Size { cols: c, rows: r });
        assert_eq!(t.fuzzball.gfx.placements.len(), n);
    }
}

/// An image anchored on the empty line right below another one is never eaten
/// when the upper image shrinks (even with no spare blank rows between them).
#[test]
fn shrinking_image_never_swallows_the_next_anchor() {
    let mut t = term(40, 30);
    // pid 1: 10x5 cells fixed; pid 2 placed on the very next row (overlapping is
    // avoided by moving the cursor only one row down).
    t.feed(b"\x1bP=Fzgp;id=1,pid=1,cols=40,rows=20,move=0\x1b\\\x1b[2;1H\x1bP=Fzgp;id=2,pid=2,cols=4,rows=1,move=0\x1b\\\x1b[25;1H$ ");
    t.resize(Size { cols: 10, rows: 30 });
    let p2 = t.fuzzball.gfx.placements.iter().find(|p| p.id == 2).unwrap();
    assert_eq!(p2.row - t.lines_scrolled, 1, "pid 2 keeps its line");
}
