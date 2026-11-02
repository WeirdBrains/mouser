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

fn term(cols: usize, rows: usize) -> (TerminalParser<Sink>, Sink, Arc<Scrollback>) {
    let sb = Arc::new(Scrollback::new(ScrollbackConfig::default()));
    let sink = Sink::default();
    (TerminalParser::new(Size { cols, rows }, sb.clone(), sink.clone()), sink, sb)
}

fn lines<W: std::io::Write>(t: &TerminalParser<W>) -> Vec<String> {
    t.screen.visible_lines().into_iter().map(|l| l.trim_end().to_string()).collect()
}

#[test]
fn zsh_prompt_sp_does_not_leave_percent_or_blank_line() {
    // zsh PROMPT_SP: '%' + (cols-1) spaces + CR + ' ' + CR, then the prompt.
    let (mut t, _, _) = term(10, 4);
    t.feed(b"%         \r \r");
    t.feed(b"\x1b[K~ \xe2\x9d\xaf ");
    assert_eq!(lines(&t), vec!["~ ❯", "", "", ""]);
    assert_eq!(t.screen.cursor.y, 0);
}

#[test]
fn autowrap_is_deferred_to_next_printable() {
    let (mut t, _, _) = term(5, 3);
    t.feed(b"abcde");
    assert_eq!(t.screen.cursor.y, 0);
    t.feed(b"f");
    assert_eq!(lines(&t), vec!["abcde", "f", ""]);
}

#[test]
fn escape_sequences_split_across_reads() {
    let (mut t, _, _) = term(10, 3);
    t.feed(b"hello\x1b[");
    t.feed(b"1;1Hj");
    assert_eq!(lines(&t)[0], "jello");
}

#[test]
fn cursor_movement_and_erase() {
    let (mut t, _, _) = term(10, 3);
    t.feed(b"0123456789\r\n abc\x1b[2D\x1b[K");
    assert_eq!(lines(&t), vec!["0123456789", " a", ""]);
    t.feed(b"\x1b[1;3H\x1b[2X");
    assert_eq!(lines(&t)[0], "01  456789");
    t.feed(b"\x1b[2J");
    assert!(lines(&t).iter().all(|l| l.is_empty()));
}

#[test]
fn scrolled_lines_go_to_scrollback() {
    let (mut t, _, sb) = term(10, 2);
    t.feed(b"one\r\ntwo\r\nthree");
    assert_eq!(lines(&t), vec!["two", "three"]);
    assert_eq!(sb.total_lines(), 1);
    assert_eq!(sb.get_lines(0, 1), vec!["one".to_string()]);
}

#[test]
fn cursor_position_report() {
    let (mut t, sink, _) = term(10, 5);
    t.feed(b"\x1b[3;4H\x1b[6n");
    assert_eq!(&*sink.0.lock().unwrap(), b"\x1b[3;4R");
}

#[test]
fn alt_screen_restores_main_screen() {
    let (mut t, _, _) = term(10, 3);
    t.feed(b"main");
    t.feed(b"\x1b[?1049h\x1b[H\x1b[2Jvim");
    assert_eq!(lines(&t)[0], "vim");
    t.feed(b"\x1b[?1049l");
    assert_eq!(lines(&t)[0], "main");
}

#[test]
fn zsh_tab_then_backspace_moves_back_over_the_tabs() {
    // What zsh echoes for Tab, Tab, Backspace, Backspace on an empty line.
    let (mut t, _, _) = term(40, 4);
    t.feed(b"$ ");
    t.feed(b"      ");
    t.feed(b"        ");
    assert_eq!(t.screen.cursor.x, 16);
    t.feed(b"\x1b[8D");
    assert_eq!(t.screen.cursor.x, 8);
    t.feed(b"\x08\x08\x08\x08\x08\x08");
    assert_eq!(t.screen.cursor.x, 2);
    t.feed(b"X");
    assert_eq!(lines(&t)[0], "$ X");
}
