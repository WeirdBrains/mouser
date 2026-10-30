//! Feed a byte capture into the emulator and print the screen (debug aid).
//! `cargo run -p mouser-core --example feed -- FILE COLS ROWS`
use mouser_core::parser::TerminalParser;
use mouser_core::scrollback::{Scrollback, ScrollbackConfig};
use mouser_core::types::Size;
use std::sync::Arc;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");
    let (cols, rows) = (a[2].parse().unwrap(), a[3].parse().unwrap());
    let sb = Arc::new(Scrollback::new(ScrollbackConfig::default()));
    let mut t = TerminalParser::new(Size { cols, rows }, sb, std::io::sink());
    t.feed(&bytes);
    for y in 0..rows {
        println!("{:2}|{}", y, t.screen.row_string(y));
    }
    println!("cursor {:?}", t.screen.cursor);
}
