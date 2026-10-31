//! Re-wrapping on resize: main screen, scrollback, cursor, wide characters,
//! attributes and images following their text.

use mouser_core::grid::attr;
use mouser_core::parser::TerminalParser;
use mouser_core::scrollback::{Scrollback, ScrollbackConfig};
use mouser_core::types::Size;
use std::sync::Arc;

fn term(cols: usize, rows: usize) -> TerminalParser<std::io::Sink> {
    let cfg = ScrollbackConfig { chunk_lines: 4, ..Default::default() };
    let mut t = TerminalParser::new(Size { cols, rows }, Arc::new(Scrollback::new(cfg)), std::io::sink());
    t.cell_px = (10, 20);
    t
}

/// Every line (scrollback, then screen rows up to the last non-empty one), with
/// soft-wrapped rows joined: the logical text.
fn logical(t: &TerminalParser<std::io::Sink>) -> Vec<String> {
    let sb = &t.scrollback;
    let mut out: Vec<String> = Vec::new();
    let mut cont = false;
    let mut push = |s: String, wrapped: bool| {
        if cont {
            out.last_mut().unwrap().push_str(&s);
        } else {
            out.push(s);
        }
        cont = wrapped;
    };
    let n = sb.total_lines();
    for (i, l) in sb.get_lines(0, n).into_iter().enumerate() {
        push(l, sb.is_wrapped(i));
    }
    for y in 0..t.screen.size.rows {
        push(t.screen.row_string(y), t.screen.row_wrapped(y));
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out
}

fn screen(t: &TerminalParser<std::io::Sink>) -> Vec<String> {
    (0..t.screen.size.rows).map(|y| t.screen.row_string(y)).collect()
}

fn cursor(t: &TerminalParser<std::io::Sink>) -> (usize, usize) {
    (t.screen.cursor.x, t.screen.cursor.y)
}

fn resize(t: &mut TerminalParser<std::io::Sink>, cols: usize, rows: usize) {
    t.resize(Size { cols, rows });
}

const LONG: &str = "The quick brown fox jumps over the lazy dog 0123456789";

#[test]
fn soft_wrapped_lines_rejoin_and_split_hard_breaks_stay() {
    let mut t = term(20, 12);
    t.feed(format!("{LONG}\r\nshort\r\n\r\nend").as_bytes());
    let before = logical(&t);
    assert_eq!(before, vec![LONG.to_string(), "short".into(), "".into(), "end".into()]);
    resize(&mut t, 60, 12);
    assert_eq!(screen(&t)[..4], [LONG.to_string(), "short".into(), "".into(), "end".into()]);
    resize(&mut t, 7, 12);
    assert_eq!(logical(&t), before);
    assert_eq!(t.screen.row_string(0), "The qui");
    assert!(t.screen.row_wrapped(0));
}

#[test]
fn narrow_wide_narrow_round_trip_is_identity() {
    let mut t = term(20, 8);
    t.feed(format!("{LONG}\r\nline two\r\n{LONG}{LONG}\r\n$ ").as_bytes());
    let (s0, c0, l0) = (screen(&t), cursor(&t), logical(&t));
    // Widening and back: the same screen and cursor.
    for c in [33, 80, 20] {
        resize(&mut t, c, 8);
        assert_eq!(logical(&t), l0, "{c} cols");
    }
    assert_eq!(screen(&t), s0);
    assert_eq!(cursor(&t), c0);
    // Through narrower widths rows go to the scrollback; the text is unchanged
    // and the cursor is still after "$ ".
    for c in [9, 33, 5, 80, 20] {
        resize(&mut t, c, 8);
        assert_eq!(logical(&t), l0, "{c} cols");
    }
    let (x, y) = cursor(&t);
    assert_eq!((x, t.screen.row_string(y)), (2, "$".to_string()));
}

#[test]
fn cursor_stays_on_its_character() {
    let mut t = term(20, 6);
    t.feed(b"$ ");
    t.feed(&[b'x'; 28]);
    // 30 columns of text: row 1, col 10.
    assert_eq!(cursor(&t), (10, 1));
    resize(&mut t, 40, 6);
    assert_eq!(cursor(&t), (30, 0));
    resize(&mut t, 10, 6);
    // Exactly three full rows: the cursor waits at the end (pending wrap).
    assert_eq!(cursor(&t), (9, 2));
    t.feed(b"Y");
    assert_eq!(t.screen.row_string(3), "Y");
    assert_eq!(logical(&t)[0], format!("$ {}Y", "x".repeat(28)));
}

#[test]
fn cursor_mid_line_and_typing_continues() {
    let mut t = term(20, 6);
    t.feed(LONG.as_bytes());
    t.feed(b"\x1b[1;5H"); // on "quick"
    resize(&mut t, 7, 6);
    assert_eq!(cursor(&t), (4, 0));
    resize(&mut t, 3, 6);
    assert_eq!(cursor(&t), (1, 1));
}

#[test]
fn wide_characters_never_split() {
    let mut t = term(20, 6);
    let text = format!("a{}", "日".repeat(12));
    t.feed(text.as_bytes());
    for cols in [20, 11, 6, 7, 3, 25, 20] {
        resize(&mut t, cols, 6);
        assert_eq!(logical(&t), vec![text.clone()], "{cols} columns");
        for y in 0..6 {
            let row = t.screen.row(y);
            for (x, c) in row.iter().enumerate() {
                if c.style.attrs & attr::WIDE != 0 {
                    assert!(x + 1 < cols && row[x + 1].style.attrs & attr::WIDE_CONT != 0, "{cols} cols row {y} col {x}");
                }
            }
        }
    }
    // "a" + 9 wide on 20 columns = 19, the 10th wraps early leaving a pad cell.
    resize(&mut t, 20, 6);
    assert_eq!(t.screen.row(0)[19].style.attrs & attr::WRAP_PAD, attr::WRAP_PAD);
    assert_eq!(t.screen.row_string(0), format!("a{}", "日".repeat(9)));
}

#[test]
fn combining_marks_attributes_and_links_survive() {
    let mut t = term(10, 6);
    t.feed("\x1b[1;31m\x1b]8;;https://x.test\x1b\\e\u{301}abcdefghijkl\x1b]8;;\x1b\\\x1b[0m".as_bytes());
    resize(&mut t, 4, 6);
    let c = t.screen.row(0)[0];
    assert_eq!((c.ch, c.comb[0]), ('e', '\u{301}'));
    let last = t.screen.row(3)[0];
    assert_eq!(last.ch, 'l');
    assert_ne!(last.style.attrs & attr::BOLD, 0);
    assert_ne!(last.style.link, 0, "hyperlink kept");
    assert_eq!(last.style.fg, c.style.fg);
    resize(&mut t, 30, 6);
    assert_eq!(t.screen.row_string(0), "e\u{301}abcdefghijkl");
}

#[test]
fn scrollback_is_rewrapped_and_continues_into_the_screen() {
    let mut t = term(20, 3);
    for i in 0..6 {
        t.feed(format!("{i}:{LONG}\r\n").as_bytes());
    }
    t.feed(b"$ ");
    let l0 = logical(&t);
    assert_eq!(l0.len(), 7);
    let sb0 = t.scrollback.total_lines();
    resize(&mut t, 80, 3);
    assert_eq!(logical(&t), l0);
    assert!(t.scrollback.total_lines() < sb0, "fewer rows when wider");
    assert!(!(0..t.scrollback.total_lines()).any(|i| t.scrollback.is_wrapped(i)));
    resize(&mut t, 20, 3);
    assert_eq!(logical(&t), l0);
    assert_eq!(t.scrollback.total_lines(), sb0);
    assert_eq!(t.screen.row_string(2), "$");
    assert_eq!(cursor(&t), (2, 2));
}

#[test]
fn lines_scrolled_and_scrollback_stay_consistent() {
    let mut t = term(20, 3);
    for i in 0..10 {
        t.feed(format!("{i}:{LONG}\r\n").as_bytes());
    }
    let first_abs = t.lines_scrolled - t.scrollback.total_lines() as i64;
    resize(&mut t, 50, 3);
    assert_eq!(t.lines_scrolled - t.scrollback.total_lines() as i64, first_abs, "oldest line keeps its absolute row");
}

#[test]
fn alt_screen_is_not_rewrapped_but_the_main_screen_is() {
    let mut t = term(20, 6);
    t.feed(LONG.as_bytes());
    t.feed(b"\x1b[?1049h\x1b[H");
    t.feed(LONG.as_bytes());
    resize(&mut t, 10, 6);
    assert_eq!(t.screen.row_string(0), LONG[..10].trim_end(), "alt screen cut, not re-wrapped");
    assert_eq!(t.screen.row_string(1), LONG[20..30].trim_end());
    t.feed(b"\x1b[?1049l");
    assert_eq!(logical(&t), vec![LONG.to_string()]);
    assert_eq!(t.screen.row_string(1), &LONG[10..20]);
}

fn fit_image(t: &mut TerminalParser<std::io::Sink>) {
    // 2x2 px image; fit=w on cells 10x20 is `cols` wide and cols/2 rows.
    t.feed(b"\x1bP=Fzgu;id=1,fmt=rgba,w=2,h=2,size=16,len=24:AAAAAAAAAAAAAAAAAAAAAA==\x1b\\");
    t.feed(b"\x1bP=Fzgp;id=1,pid=1,cols=1,fit=w\x1b\\\r\n");
}

fn image(t: &TerminalParser<std::io::Sink>) -> (i64, u32, u32) {
    let p = &t.fuzzball.gfx.placements[0];
    (p.row - t.lines_scrolled, p.cols, p.rows)
}

fn row_of(t: &TerminalParser<std::io::Sink>, text: &str) -> Option<usize> {
    (0..t.screen.size.rows).find(|&y| t.screen.row_string(y) == text)
}

#[test]
fn fitted_image_makes_room_and_text_below_moves() {
    let mut t = term(20, 40);
    t.feed(format!("{LONG}\r\n").as_bytes());
    fit_image(&mut t);
    t.feed(b"after");
    // LONG takes 3 rows, image at row 3, 20x10 cells, "after" right below.
    assert_eq!(image(&t), (3, 20, 10));
    assert_eq!(row_of(&t, "after"), Some(13));
    resize(&mut t, 30, 40);
    // LONG now 2 rows; image 30x15; "after" below it, not under it.
    assert_eq!(image(&t), (2, 30, 15));
    assert_eq!(row_of(&t, "after"), Some(17));
    resize(&mut t, 10, 40);
    assert_eq!(image(&t), (6, 10, 5));
    assert_eq!(row_of(&t, "after"), Some(11));
    resize(&mut t, 20, 40);
    assert_eq!(image(&t), (3, 20, 10));
    assert_eq!(row_of(&t, "after"), Some(13));
    assert_eq!(cursor(&t), (5, 13));
}

#[test]
fn image_in_scrollback_follows_its_line() {
    let mut t = term(20, 5);
    t.feed(format!("{LONG}\r\n").as_bytes());
    fit_image(&mut t);
    t.feed(b"after\r\n");
    for i in 0..5 {
        t.feed(format!("{i}\r\n").as_bytes());
    }
    let abs = t.fuzzball.gfx.placements[0].row;
    let first = t.lines_scrolled - t.scrollback.total_lines() as i64;
    assert_eq!(abs - first, 3, "anchored below the three rows of LONG");
    resize(&mut t, 30, 5);
    let p = &t.fuzzball.gfx.placements[0];
    let first = t.lines_scrolled - t.scrollback.total_lines() as i64;
    assert_eq!((p.row - first, p.cols, p.rows), (2, 30, 15));
    let after = (0..t.scrollback.total_lines()).find(|&i| t.scrollback.get_lines(i, 1)[0] == "after").unwrap();
    assert_eq!(after as i64, 2 + 15, "text below the image moved down");
}

#[test]
fn kitty_placement_with_explicit_size_moves_with_text_and_keeps_its_size() {
    let mut t = term(20, 20);
    t.feed(format!("{LONG}\r\n").as_bytes());
    // 2x2 RGBA, 4x2 cells, cursor moves below.
    t.feed(b"\x1b_Ga=T,f=32,s=2,v=2,i=9,c=4,r=2,q=2;AAAAAAAAAAAAAAAAAAAAAA==\x1b\\\r\nafter");
    let p = &t.kitty_gfx.gfx.placements[0];
    assert_eq!((p.row - t.lines_scrolled, p.cols, p.rows), (3, 4, 2));
    resize(&mut t, 60, 20);
    let p = &t.kitty_gfx.gfx.placements[0];
    assert_eq!((p.row - t.lines_scrolled, p.cols, p.rows), (1, 4, 2));
    assert_eq!(row_of(&t, "after"), Some(3));
}

#[test]
fn rows_only_resize_keeps_old_behaviour() {
    let mut t = term(20, 6);
    t.feed(format!("{LONG}\r\n$ ").as_bytes());
    let l0 = logical(&t);
    resize(&mut t, 20, 3);
    resize(&mut t, 20, 10);
    assert_eq!(logical(&t), l0);
}

/// `cargo test --release -p mouser-core --test reflow_tests -- --ignored --nocapture`
#[test]
#[ignore]
fn perf_large_scrollback() {
    let cfg = ScrollbackConfig::default();
    let mut t = TerminalParser::new(Size { cols: 120, rows: 50 }, Arc::new(Scrollback::new(cfg)), std::io::sink());
    let mut buf = Vec::new();
    for i in 0..200_000 {
        if i % 3 == 0 {
            buf.extend_from_slice(format!("{i}: {LONG} {LONG} {LONG}\r\n").as_bytes());
        } else {
            buf.extend_from_slice(format!("{i}: short line\r\n").as_bytes());
        }
    }
    t.feed(&buf);
    let n = t.scrollback.total_lines();
    for cols in [80, 200, 120] {
        let s = std::time::Instant::now();
        resize(&mut t, cols, 50);
        eprintln!("reflow {n} scrollback lines -> {cols} cols: {:?} ({} lines now)", s.elapsed(), t.scrollback.total_lines());
    }
}
