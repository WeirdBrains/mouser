//! Fuzzball placement lifecycle (graphics.md §4): scrollback eviction, ED 3,
//! resize, scroll regions and the alternate screen.

use mouser_core::parser::TerminalParser;
use mouser_core::scrollback::{Scrollback, ScrollbackConfig};
use mouser_core::types::Size;
use std::sync::Arc;

fn term_with(max_lines: usize) -> TerminalParser<std::io::Sink> {
    let cfg = ScrollbackConfig { max_scrollback_lines: max_lines, chunk_lines: 2, ..Default::default() };
    let sb = Arc::new(Scrollback::new(cfg));
    let mut t = TerminalParser::new(Size { cols: 20, rows: 6 }, sb, std::io::sink());
    t.cell_px = (10, 20);
    // 2x2 RGBA image, id 1.
    t.feed(b"\x1bP=Fzgu;id=1,fmt=rgba,w=2,h=2,size=16,len=24:AAAAAAAAAAAAAAAAAAAAAA==\x1b\\");
    assert_eq!(t.fuzzball.gfx.images.len(), 1);
    t
}

fn term() -> TerminalParser<std::io::Sink> {
    term_with(100_000)
}

/// Place image 1 as `pid` at 1-based (row, col), `cols`x`rows` cells, cursor kept.
fn place(t: &mut TerminalParser<std::io::Sink>, pid: u32, row: usize, col: usize, cols: u32, rows: u32) {
    t.feed(format!("\x1b[{row};{col}H\x1bP=Fzgp;id=1,pid={pid},cols={cols},rows={rows},move=0\x1b\\").as_bytes());
}

fn pids(t: &TerminalParser<std::io::Sink>) -> Vec<u32> {
    let mut v: Vec<u32> = t.fuzzball.gfx.placements.iter().map(|p| p.id).collect();
    v.sort();
    v
}

fn screen_row(t: &TerminalParser<std::io::Sink>, pid: u32) -> i64 {
    let p = t.fuzzball.gfx.placements.iter().find(|p| p.id == pid).expect("placement");
    p.row - t.lines_scrolled
}

fn scroll(t: &mut TerminalParser<std::io::Sink>, n: usize) {
    t.feed(b"\x1b[6;1H");
    t.feed(&b"\n".repeat(n));
}

#[test]
fn placement_survives_while_its_row_is_in_scrollback() {
    let mut t = term_with(8);
    place(&mut t, 1, 1, 1, 2, 2);
    scroll(&mut t, 6);
    assert_eq!(pids(&t), vec![1]);
    assert_eq!(screen_row(&t, 1), -6);
}

#[test]
fn eviction_at_the_scrollback_limit_removes_the_placement() {
    let mut t = term_with(4);
    place(&mut t, 1, 1, 1, 2, 2);
    place(&mut t, 2, 6, 1, 2, 1);
    scroll(&mut t, 7);
    // Row of pid 1 dropped out with the oldest chunk; pid 2 is still saved.
    assert!(t.lines_scrolled - t.scrollback.total_lines() as i64 > 0);
    assert_eq!(pids(&t), vec![2]);
    // Image data stays (it can be placed again) until deleted or evicted for memory.
    assert_eq!(t.fuzzball.gfx.images.len(), 1);
    assert_eq!(t.fuzzball.gfx.mem_used(), 16);
}

#[test]
fn eviction_uses_the_anchor_row_for_tall_placements() {
    let mut t = term_with(4);
    // Anchor at row 1, spans 6 rows: removed when its anchor row is evicted,
    // even though its lower rows are still saved.
    place(&mut t, 1, 1, 1, 2, 6);
    scroll(&mut t, 5);
    assert_eq!(pids(&t), vec![1]);
    scroll(&mut t, 2);
    assert!(pids(&t).is_empty());
}

#[test]
fn clear_scrollback_removes_saved_placements_only() {
    let mut t = term();
    place(&mut t, 1, 1, 1, 2, 1);
    place(&mut t, 2, 6, 1, 2, 1);
    scroll(&mut t, 2);
    assert_eq!(screen_row(&t, 1), -2);
    t.feed(b"\x1b[3J");
    assert_eq!(t.scrollback.total_lines(), 0);
    assert_eq!(pids(&t), vec![2], "on-screen placement and text stay");
    assert_eq!(screen_row(&t, 2), 3);
}

#[test]
fn ed3_keeps_the_visible_text() {
    let mut t = term();
    t.feed(b"hello");
    t.feed(b"\x1b[3J");
    assert!(t.screen.visible_lines()[0].starts_with("hello"));
}

#[test]
fn narrower_resize_drops_cut_off_anchors() {
    let mut t = term();
    place(&mut t, 1, 2, 3, 4, 1);
    place(&mut t, 2, 3, 15, 4, 1);
    t.resize(Size { cols: 10, rows: 6 });
    // Anchor col 14 is gone; col 2 stays (it may be clipped at the edge).
    assert_eq!(pids(&t), vec![1]);
    assert_eq!(screen_row(&t, 1), 1);
}

#[test]
fn wider_resize_keeps_placements_in_place() {
    let mut t = term();
    place(&mut t, 1, 2, 3, 4, 1);
    let before = t.fuzzball.gfx.placements[0].clone();
    t.resize(Size { cols: 40, rows: 6 });
    let after = &t.fuzzball.gfx.placements[0];
    assert_eq!((after.row, after.col), (before.row, before.col));
}

#[test]
fn shorter_resize_moves_rows_into_scrollback_with_their_placements() {
    let mut t = term();
    place(&mut t, 1, 1, 1, 2, 1);
    place(&mut t, 2, 6, 1, 2, 1);
    // Cursor on the last row: shrinking to 3 rows pushes the top 3 rows to scrollback.
    t.feed(b"\x1b[6;1H");
    t.resize(Size { cols: 20, rows: 3 });
    assert_eq!(t.lines_scrolled, 3);
    assert_eq!(screen_row(&t, 1), -3, "moved to scrollback with its line");
    assert_eq!(screen_row(&t, 2), 2, "stays on its line");
}

#[test]
fn shorter_resize_drops_rows_below_the_screen() {
    let mut t = term();
    place(&mut t, 1, 6, 1, 2, 1);
    t.feed(b"\x1b[1;1H");
    t.resize(Size { cols: 20, rows: 3 });
    assert!(pids(&t).is_empty(), "its line was cut off");
}

#[test]
fn alt_screen_placements_scroll_and_drop_off_the_top() {
    let mut t = term();
    t.feed(b"\x1b[?1049h");
    place(&mut t, 1, 2, 1, 2, 1);
    scroll(&mut t, 1);
    assert_eq!(screen_row(&t, 1), 0, "moved up with its text");
    scroll(&mut t, 1);
    assert!(pids(&t).is_empty(), "alt screen has no scrollback");
}

#[test]
fn alt_screen_resize_and_leave() {
    let mut t = term();
    place(&mut t, 1, 2, 1, 2, 1);
    t.feed(b"\x1b[?1049h");
    place(&mut t, 2, 6, 1, 2, 1);
    t.feed(b"\x1b[6;1H");
    t.resize(Size { cols: 20, rows: 4 });
    // Alt content moved up by 2 to keep the cursor row; main screen was blanked.
    assert_eq!(pids(&t), vec![2]);
    assert_eq!(screen_row(&t, 2), 3);
    t.feed(b"\x1b[?1049l");
    assert!(pids(&t).is_empty());
}

#[test]
fn scroll_regions_move_and_drop_placements() {
    let mut t = term();
    place(&mut t, 1, 3, 1, 2, 1);
    place(&mut t, 2, 6, 1, 2, 1);
    // Region rows 2..5; scroll it up once (SU), then down twice (SD).
    t.feed(b"\x1b[2;5r\x1b[S");
    assert_eq!(screen_row(&t, 1), 1);
    assert_eq!(screen_row(&t, 2), 5, "outside the region");
    t.feed(b"\x1b[3T");
    assert_eq!(screen_row(&t, 1), 4);
    t.feed(b"\x1b[T");
    assert_eq!(pids(&t), vec![2], "pushed past the region bottom");
}

#[test]
fn delete_and_insert_lines_move_placements() {
    let mut t = term();
    place(&mut t, 1, 2, 1, 2, 1);
    place(&mut t, 2, 4, 1, 2, 1);
    t.feed(b"\x1b[2;1H\x1b[M");
    assert_eq!(pids(&t), vec![2], "deleted with its line");
    assert_eq!(screen_row(&t, 2), 2);
    t.feed(b"\x1b[1;1H\x1b[2L");
    assert_eq!(screen_row(&t, 2), 4);
}

#[test]
fn abandoned_upload_expires_on_the_timer() {
    use mouser_core::fuzzball::UPLOAD_TIMEOUT;
    use std::time::Instant;
    let mut t = term();
    // First chunk of a 2x2 upload (more=1), never finished.
    t.feed(b"\x1bP=Fzgu;id=9,fmt=rgba,w=2,h=2,size=16,len=8,more=1:AAAAAAAA\x1b\\");
    assert!(t.fuzzball.upload_pending());
    let now = Instant::now();
    let deadline = t.fuzzball.expire_upload(now).expect("still pending");
    assert!(deadline <= now + UPLOAD_TIMEOUT);
    assert!(t.fuzzball.upload_pending());
    assert_eq!(t.fuzzball.expire_upload(deadline), None);
    assert!(!t.fuzzball.upload_pending());
    // A late chunk does not resurrect it: it starts a new upload, which lacks fmt.
    t.feed(b"\x1bP=Fzgu;id=9,len=8:AAAAAAAA\x1b\\");
    assert!(!t.fuzzball.upload_pending());
    assert!(!t.fuzzball.gfx.images.contains_key(&9));
}
