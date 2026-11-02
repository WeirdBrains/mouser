//! Fuzzball `s` (docs/protocol/shell.md) on the terminal side, and the
//! no-integration fallback that erases a typed `cd`.

use mouser_core::fuzzball::{self, pct_decode, pct_encode};
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

fn term(cols: usize, rows: usize) -> (TerminalParser<Sink>, Sink) {
    let sink = Sink::default();
    (TerminalParser::new(Size { cols, rows }, Arc::new(Scrollback::new(ScrollbackConfig::default())), sink.clone()), sink)
}

#[test]
fn percent_encoding_round_trips_every_byte() {
    let all: Vec<u8> = (0..=255).collect();
    let e = pct_encode(&all);
    assert!(!e.contains([';', ',', '=', ' ', '\x1b']));
    assert_eq!(pct_decode(&e).unwrap(), all);
    assert_eq!(pct_encode(b"/a b/c,d"), "/a%20b/c%2Cd");
    assert!(pct_decode("%4").is_none());
    assert_eq!(fuzzball::shell_cd(3, b"/tmp/x y"), b"\x1bP=Fzsc;id=3,path=/tmp/x%20y\x1b\\".to_vec());
}

#[test]
fn advertisement_is_recorded_and_withdrawn() {
    let (mut t, sink) = term(40, 5);
    assert!(t.fuzzball.shell.is_none());
    t.feed(b"\x1bP=Fzsa;v=0,shell=zsh,caps=cwd\x1b\\");
    let s = t.fuzzball.shell.clone().unwrap();
    assert!(s.has("cwd") && !s.has("run") && s.shell == "zsh");
    assert!(sink.0.lock().unwrap().is_empty(), "no reply without id");
    t.feed(b"\x1bP=Fzsa;v=0,caps=\x1b\\");
    assert!(t.fuzzball.shell.is_none(), "withdrawn while a command runs");
    t.feed(b"\x1bP=Fzsa;id=2,v=9,caps=cwd\x1b\\");
    assert!(t.fuzzball.shell.is_none());
    assert!(String::from_utf8_lossy(&sink.0.lock().unwrap()).contains("Fzsr;id=2,ok=0,err=EVERSION"));
    t.feed(b"\x1bP=Fzcq;id=1\x1b\\");
    assert!(String::from_utf8_lossy(&sink.0.lock().unwrap()).contains("sub=gkps"));
}

#[test]
fn replies_from_the_shell_are_queued_and_never_answered() {
    let (mut t, sink) = term(40, 5);
    t.feed(b"\x1bP=Fzsr;id=7,ok=1\x1b\\\x1bP=Fzsr;id=8,ok=0,err=ENOENT,msg=cannot%20cd%20to%20%2Fx\x1b\\");
    assert_eq!(t.fuzzball.shell_replies, vec![(7, true, String::new()), (8, false, "cannot cd to /x".into())]);
    assert!(sink.0.lock().unwrap().is_empty());
}

#[test]
fn erase_rows_leaves_one_prompt() {
    let (mut t, _) = term(30, 8);
    // OSC 133 A marks each prompt start (two-line prompt).
    t.feed(b"old output\r\n\x1b]133;A\x07[~]\r\n$ cd -- '/srv'\r\n\x1b]133;A\x07[/srv]\r\n$ ");
    let start = 1;
    let end = t.prompt_start();
    assert_eq!(end, 3);
    assert!(t.erase_rows(start, end));
    let l = t.screen.visible_lines();
    assert_eq!(l[0].trim_end(), "old output");
    assert_eq!(l[1].trim_end(), "[/srv]");
    assert_eq!(l[2].trim_end(), "$");
    assert!(l[3].trim().is_empty());
    assert_eq!((t.screen.cursor.y, t.screen.cursor.x), (2, 2));
    assert_eq!(t.prompt_start(), 1, "the mark moved up with the prompt");
    t.feed(b"ls");
    assert_eq!(t.screen.visible_lines()[2].trim_end(), "$ ls");
    // Nothing to erase, or rows below the cursor: refused.
    assert!(!t.erase_rows(1, 1));
    assert!(!t.erase_rows(1, 7));
}

#[test]
fn erase_rows_is_refused_on_the_alternate_screen() {
    let (mut t, _) = term(30, 8);
    t.feed(b"a\r\nb\r\n\x1b[?1049h\r\n\r\n");
    assert!(!t.erase_rows(0, 1));
}
