//! VT/xterm compliance checks, one area per test.
use mouser_core::grid::{CursorShape, attr};
use mouser_core::input::{self, MouseButton, MouseEvent, MouseKind};
use mouser_core::parser::{MouseMode, TermEvent, TerminalParser, parse_color};
use mouser_core::scrollback::{Scrollback, ScrollbackConfig};
use mouser_core::types::Size;
use mouser_core::{Color, Rgb};
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

fn term(cols: usize, rows: usize) -> (TerminalParser<Out>, Out) {
    let out = Out::default();
    let sb = Arc::new(Scrollback::new(ScrollbackConfig::default()));
    (TerminalParser::new(Size { cols, rows }, sb, out.clone()), out)
}

fn cell(t: &TerminalParser<Out>, x: usize, y: usize) -> mouser_core::Cell {
    t.screen.row(y)[x]
}

#[test]
fn sgr_16_256_truecolor_and_reset() {
    let (mut t, _) = term(20, 2);
    t.feed(b"\x1b[31;42ma\x1b[91;104mb\x1b[38;5;196;48;5;21mc\x1b[38;2;1;2;3;48:2::4:5:6md\x1b[0me");
    assert_eq!(cell(&t, 0, 0).style.fg, Color::Indexed(1));
    assert_eq!(cell(&t, 0, 0).style.bg, Color::Indexed(2));
    assert_eq!(cell(&t, 1, 0).style.fg, Color::Indexed(9));
    assert_eq!(cell(&t, 1, 0).style.bg, Color::Indexed(12));
    assert_eq!(cell(&t, 2, 0).style.fg, Color::Indexed(196));
    assert_eq!(cell(&t, 2, 0).style.bg, Color::Indexed(21));
    assert_eq!(cell(&t, 3, 0).style.fg, Color::Rgb(1, 2, 3));
    assert_eq!(cell(&t, 3, 0).style.bg, Color::Rgb(4, 5, 6));
    assert_eq!(cell(&t, 4, 0).style, Default::default());
}

#[test]
fn sgr_attributes_and_underline_styles() {
    let (mut t, _) = term(20, 2);
    t.feed(b"\x1b[1;2;3;5;7;8;9;53ma\x1b[22;23;25;27;28;29;55mb\x1b[4:3;58:2::9:8:7mc\x1b[21md\x1b[4:5me\x1b[24;59mf");
    let a = cell(&t, 0, 0).style.attrs;
    for b in [attr::BOLD, attr::DIM, attr::ITALIC, attr::BLINK, attr::INVERSE, attr::HIDDEN, attr::STRIKE, attr::OVERLINE] {
        assert!(a & b != 0, "bit {b}");
    }
    assert_eq!(cell(&t, 1, 0).style.attrs, 0);
    assert_eq!(cell(&t, 2, 0).style.underline, 3);
    assert_eq!(cell(&t, 2, 0).style.ul_color, Color::Rgb(9, 8, 7));
    assert_eq!(cell(&t, 3, 0).style.underline, 2);
    assert_eq!(cell(&t, 4, 0).style.underline, 5);
    assert_eq!(cell(&t, 5, 0).style.underline, 0);
    assert_eq!(cell(&t, 5, 0).style.ul_color, Color::Default);
}

#[test]
fn decrqss_reports_sgr() {
    let (mut t, out) = term(20, 6);
    t.feed(b"\x1b[1;4:3;38;5;100;48;2;1;2;3m\x1bP$qm\x1b\\");
    assert_eq!(out.take(), "\x1bP1$r0;1;4:3;38;5;100;48;2;1;2;3m\x1b\\");
    t.feed(b"\x1b[2;5r\x1bP$qr\x1b\\");
    assert_eq!(out.take(), "\x1bP1$r2;5r\x1b\\");
    t.feed(b"\x1b[6 q\x1bP$q q\x1b\\");
    assert_eq!(out.take(), "\x1bP1$r6 q\x1b\\");
    t.feed(b"\x1bP$qzz\x1b\\");
    assert_eq!(out.take(), "\x1bP0$r\x1b\\");
}

#[test]
fn wide_and_combining_characters() {
    let (mut t, _) = term(6, 2);
    t.feed("a漢b".as_bytes());
    assert_eq!(cell(&t, 1, 0).ch, '漢');
    assert!(cell(&t, 1, 0).style.attrs & attr::WIDE != 0);
    assert!(cell(&t, 2, 0).style.attrs & attr::WIDE_CONT != 0);
    assert_eq!(cell(&t, 3, 0).ch, 'b');
    assert_eq!(t.screen.cursor.x, 4);
    // Combining acute attaches to the previous cell without advancing.
    t.feed("e\u{301}".as_bytes());
    assert_eq!(cell(&t, 4, 0).comb[0], '\u{301}');
    assert_eq!(t.screen.cursor.x, 5);
    assert_eq!(t.screen.row_string(0), "a漢be\u{301}");
    // A wide char that doesn't fit wraps to the next line.
    t.feed("漢".as_bytes());
    assert_eq!(cell(&t, 0, 1).ch, '漢');
    // Emoji and ZWJ sequences: the emoji takes 2 cells, joiners attach.
    let (mut t, _) = term(10, 1);
    t.feed("👍🏽x".as_bytes());
    assert!(cell(&t, 0, 0).style.attrs & attr::WIDE != 0);
    t.feed("\x1b[H".as_bytes());
    t.feed("👩\u{200d}💻".as_bytes());
    assert_eq!(cell(&t, 0, 0).comb[0], '\u{200d}');
    // Overwriting half of a wide char blanks the other half.
    let (mut t, _) = term(6, 1);
    t.feed("漢\x1b[2Gx".as_bytes());
    assert_eq!(cell(&t, 0, 0).ch, ' ');
    assert_eq!(cell(&t, 1, 0).ch, 'x');
}

#[test]
fn editing_ech_rep_ich_dch_il_dl() {
    let (mut t, _) = term(8, 4);
    t.feed(b"abcdefgh\x1b[1;3H\x1b[2X");
    assert_eq!(t.screen.row_string(0), "ab  efgh");
    t.feed(b"\x1b[1;1H\x1b[2@");
    assert_eq!(t.screen.row_string(0), "  ab  ef");
    t.feed(b"\x1b[3P");
    assert_eq!(t.screen.row_string(0), "b  ef");
    t.feed(b"\x1b[2;1Hz\x1b[3b");
    assert_eq!(t.screen.row_string(1), "zzzz");
    t.feed(b"\x1b[1;1H\x1b[L");
    assert_eq!(t.screen.row_string(0), "");
    assert_eq!(t.screen.row_string(2), "zzzz");
    t.feed(b"\x1b[M");
    assert_eq!(t.screen.row_string(1), "zzzz");
    // IRM insert mode.
    t.feed(b"\x1b[4h\x1b[2;1HQ\x1b[4l");
    assert_eq!(t.screen.row_string(1), "Qzzzz");
}

#[test]
fn erase_uses_current_background() {
    let (mut t, _) = term(4, 1);
    t.feed(b"\x1b[44m\x1b[2K");
    assert_eq!(cell(&t, 3, 0).style.bg, Color::Indexed(4));
}

#[test]
fn tab_stops_hts_tbc_cht_cbt() {
    let (mut t, _) = term(40, 1);
    t.feed(b"\t");
    assert_eq!(t.screen.cursor.x, 8);
    t.feed(b"\x1b[3g\x1b[1;5H\x1bH\x1b[1;13H\x1bH\x1b[1;1H\t");
    assert_eq!(t.screen.cursor.x, 4);
    t.feed(b"\x1b[I");
    assert_eq!(t.screen.cursor.x, 12);
    t.feed(b"\x1b[2Z");
    assert_eq!(t.screen.cursor.x, 0);
    t.feed(b"\x1b[1;13H\x1b[0g\x1b[1;5H\t");
    assert_eq!(t.screen.cursor.x, 39);
}

#[test]
fn decsc_restores_attributes_and_origin_mode() {
    let (mut t, _) = term(10, 6);
    t.feed(b"\x1b[31m\x1b[2;3H\x1b7\x1b[0m\x1b[5;5H\x1b8x");
    assert_eq!(cell(&t, 2, 1).style.fg, Color::Indexed(1));
    // DECOM: CUP is relative to the scroll region.
    t.feed(b"\x1b[3;5r\x1b[?6h\x1b[1;1Hy");
    assert_eq!(cell(&t, 0, 2).ch, 'y');
    t.feed(b"\x1b[9;1Hz");
    assert_eq!(cell(&t, 0, 4).ch, 'z');
    t.feed(b"\x1b[?6l");
}

#[test]
fn decawm_off_overwrites_last_column() {
    let (mut t, _) = term(4, 2);
    t.feed(b"\x1b[?7labcdef");
    assert_eq!(t.screen.row_string(0), "abcf");
    assert_eq!(t.screen.row_string(1), "");
    t.feed(b"\x1b[?7h\x1b[1;1Habcdef");
    assert_eq!(t.screen.row_string(1), "ef");
}

#[test]
fn alt_screens_and_cursor_style() {
    let (mut t, _) = term(5, 2);
    for (on, off) in [("\x1b[?1049h", "\x1b[?1049l"), ("\x1b[?1047h", "\x1b[?1047l"), ("\x1b[?47h", "\x1b[?47l")] {
        t.feed(b"\x1b[H\x1b[2Jmain");
        t.feed(on.as_bytes());
        assert!(t.screen.in_alt_screen());
        t.feed(b"alt");
        t.feed(off.as_bytes());
        assert_eq!(t.screen.row_string(0), "main");
    }
    t.feed(b"\x1b[4 q");
    assert_eq!(t.screen.cursor_shape, CursorShape::Underline);
    assert!(!t.screen.cursor_blink);
    t.feed(b"\x1b[5 q");
    assert_eq!(t.screen.cursor_shape, CursorShape::Bar);
    assert!(t.screen.cursor_blink);
    t.feed(b"\x1b[?25l");
    assert!(!t.screen.cursor_visible);
}

#[test]
fn private_modes_and_decrqm() {
    let (mut t, out) = term(5, 2);
    t.feed(b"\x1b[?2004h\x1b[?1004h\x1b[?1002h\x1b[?1006h\x1b[?2026h\x1b[?1h");
    let m = t.modes;
    assert!(m.bracketed_paste && m.focus_events && m.mouse_sgr && m.synchronized && m.app_cursor);
    assert_eq!(m.mouse, MouseMode::Drag);
    t.feed(b"\x1b[?2004$p\x1b[?2027$p\x1b[?9999$p\x1b[4$p");
    assert_eq!(out.take(), "\x1b[?2004;1$y\x1b[?2027;3$y\x1b[?9999;0$y\x1b[4;2$y");
    t.feed(b"\x1b[?2026l\x1b[?1002l");
    assert!(!t.modes.synchronized);
    assert_eq!(t.modes.mouse, MouseMode::Off);
}

#[test]
fn mouse_encodings() {
    let (mut t, _) = term(5, 2);
    let ev = |kind, button| MouseEvent { kind, button, col: 2, row: 1, px: 20, py: 30, shift: false, alt: false, ctrl: true };
    assert_eq!(input::encode_mouse(&ev(MouseKind::Press, MouseButton::Left), &t.modes), None);
    t.feed(b"\x1b[?1000h");
    assert_eq!(input::encode_mouse(&ev(MouseKind::Press, MouseButton::Left), &t.modes).unwrap(), b"\x1b[M0#\""); // 16+32='0', 3+32, 2+32
    assert_eq!(input::encode_mouse(&ev(MouseKind::Release, MouseButton::Left), &t.modes).unwrap(), b"\x1b[M3#\"");
    assert_eq!(input::encode_mouse(&ev(MouseKind::Motion, MouseButton::Left), &t.modes), None);
    t.feed(b"\x1b[?1006h\x1b[?1003h");
    assert_eq!(input::encode_mouse(&ev(MouseKind::Release, MouseButton::Right), &t.modes).unwrap(), b"\x1b[<18;3;2m");
    assert_eq!(input::encode_mouse(&ev(MouseKind::Motion, MouseButton::None), &t.modes).unwrap(), b"\x1b[<51;3;2M");
    assert_eq!(input::encode_mouse(&ev(MouseKind::Press, MouseButton::WheelUp), &t.modes).unwrap(), b"\x1b[<80;3;2M");
    t.feed(b"\x1b[?1016h");
    assert_eq!(input::encode_mouse(&ev(MouseKind::Press, MouseButton::Left), &t.modes).unwrap(), b"\x1b[<16;21;31M");
}

#[test]
fn focus_paste_and_cursor_keys() {
    let (mut t, _) = term(5, 2);
    assert_eq!(input::encode_focus(true, &t.modes), None);
    assert_eq!(input::paste_bytes("a\nb", &t.modes), b"a\rb");
    t.feed(b"\x1b[?1004h\x1b[?2004h\x1b[?1h");
    assert_eq!(input::encode_focus(false, &t.modes), Some(&b"\x1b[O"[..]));
    assert_eq!(input::paste_bytes("x\x1b[201~y", &t.modes), b"\x1b[200~xy\x1b[201~");
    assert_eq!(input::apply_cursor_mode(b"\x1b[A".to_vec(), &t.modes), b"\x1bOA");
    assert_eq!(input::apply_cursor_mode(b"\x1b[1;5A".to_vec(), &t.modes), b"\x1b[1;5A");
}

#[test]
fn osc_titles_cwd_marks_and_clipboard() {
    let (mut t, _) = term(10, 3);
    t.feed(b"\x1b]2;hello; world\x07\x1b]7;file://host/tmp\x1b\\\x1b]133;A\x07\x1b]52;c;aGk=\x07\x1b]52;c;?\x07\x07");
    assert_eq!(t.title, "hello; world");
    assert_eq!(t.cwd.as_deref(), Some("file://host/tmp"));
    assert_eq!(t.prompt_marks, vec![(0, 'A')]);
    assert_eq!(t.events, vec![TermEvent::Title("hello; world".into()), TermEvent::Clipboard("hi".into()), TermEvent::Bell]);
}

#[test]
fn osc8_hyperlinks() {
    let (mut t, _) = term(10, 1);
    t.feed(b"a\x1b]8;id=1;https://x.test/a;b\x1b\\link\x1b]8;;\x1b\\z");
    assert_eq!(t.link_at(0, 0), None);
    assert_eq!(t.link_at(1, 0), Some("https://x.test/a;b"));
    assert_eq!(t.link_at(4, 0), Some("https://x.test/a;b"));
    assert_eq!(t.link_at(5, 0), None);
}

#[test]
fn osc_color_query_and_set() {
    let (mut t, out) = term(10, 1);
    t.theme.bg = Rgb { r: 4, g: 0x19, b: 0x2a };
    t.feed(b"\x1b]11;?\x07");
    assert_eq!(out.take(), "\x1b]11;rgb:0404/1919/2a2a\x07");
    t.feed(b"\x1b]4;1;#ff0000;196;?\x1b\\");
    assert_eq!(out.take(), "\x1b]4;196;rgb:ffff/0000/0000\x1b\\");
    t.feed(b"\x1b]4;1;?\x07");
    assert_eq!(out.take(), "\x1b]4;1;rgb:ffff/0000/0000\x07");
    t.feed(b"\x1b]10;rgb:ff/80/00\x07\x1b]10;?\x07");
    assert_eq!(out.take(), "\x1b]10;rgb:ffff/8080/0000\x07");
    t.feed(b"\x1b]110\x07\x1b]104\x07");
    assert!(t.color_overrides.is_empty());
    assert_eq!(parse_color("rgb:f/0/fff"), Some(Rgb { r: 255, g: 0, b: 255 }));
}

#[test]
fn device_reports() {
    let (mut t, out) = term(10, 5);
    t.feed(b"\x1b[c");
    assert_eq!(out.take(), "\x1b[?62;22c");
    t.feed(b"\x1b[>c");
    assert!(out.take().starts_with("\x1b[>1;"));
    t.feed(b"\x1b[=c");
    assert_eq!(out.take(), "\x1bP!|4D4F5553\x1b\\");
    t.feed(b"\x1b[5n\x1b[3;4H\x1b[6n\x1b[?6n");
    assert_eq!(out.take(), "\x1b[0n\x1b[3;4R\x1b[?3;4R");
    t.feed(b"\x1b[>q");
    assert!(out.take().starts_with("\x1bP>|Mouser("));
    t.feed(b"\x1b[18t\x1b[16t");
    assert_eq!(out.take(), "\x1b[8;5;10t\x1b[6;18;9t");
}

#[test]
fn xtgettcap() {
    let (mut t, out) = term(10, 1);
    // "TN" = 544e, "Co" = 436f, "zz" = 7a7a
    t.feed(b"\x1bP+q544e;436f;7a7a\x1b\\");
    assert_eq!(out.take(), "\x1bP1+r544e=787465726d2d323536636f6c6f72\x1b\\\x1bP1+r436f=323536\x1b\\\x1bP0+r7a7a\x1b\\");
}

#[test]
fn fuzzball_capability_query() {
    let (mut t, out) = term(10, 1);
    t.feed(b"\x1bP=Fzcq;id=7\x1b\\");
    assert!(out.take().starts_with("\x1bP=Fzcr;id=7,v=0"));
}

#[test]
fn full_reset_clears_modes_and_style() {
    let (mut t, _) = term(10, 2);
    t.feed(b"\x1b[31m\x1b[?2004h\x1b[?1000h\x1b]4;1;#010203\x07\x1bc x");
    assert_eq!(t.modes.mouse, MouseMode::Off);
    assert!(!t.modes.bracketed_paste);
    assert!(t.color_overrides.is_empty());
    assert_eq!(cell(&t, 1, 0).style.fg, Color::Default);
}

#[test]
fn dec_special_graphics_and_shift_in_out() {
    let (mut t, _) = term(10, 2);
    t.feed(b"\x1b(0lqk\x1b(Bq\x1b)0\x0eqx\x0fq\x1b(A#");
    assert_eq!(t.screen.row_string(0), "┌─┐q─│q£");
    t.feed(b"\x1b(0\x1bc`");
    assert_eq!(t.screen.row_string(0), "`");
}

#[test]
fn decscnm_reverse_video() {
    let (mut t, out) = term(10, 2);
    t.feed(b"\x1b[?5h\x1b[?5$p");
    assert!(t.modes.reverse_video);
    assert_eq!(out.take(), "\x1b[?5;1$y");
    t.feed(b"\x1b[?5l");
    assert!(!t.modes.reverse_video);
}

#[test]
fn lnm_and_decreqtparm() {
    let (mut t, out) = term(10, 3);
    t.feed(b"ab\x1b[20hc\nd");
    assert_eq!(t.screen.row_string(1), "d");
    assert!(t.modes.newline);
    t.feed(b"\x1b[20$p\x1b[20l\x1b[x\x1b[1x");
    assert_eq!(out.take(), "\x1b[20;1$y\x1b[2;1;1;128;128;1;0x\x1b[3;1;1;128;128;1;0x");
}
