//! `mouser md`: Markdown rendered into the terminal (docs/markdown.md).
//!
//! The output is ordinary text with SGR styles and OSC 8 links, so it lands
//! in the grid and the scrollback like any other output, and works in any
//! terminal, over SSH and inside tmux. No new escape sequence is needed: a
//! program that wants Markdown shown pipes it through `mouser md -`.

use anyhow::{Context, Result};
use mouser_core::markdown;
use std::io::{IsTerminal, Read, Write};

/// When to style the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Color {
    /// Styled when stdout is a terminal.
    #[default]
    Auto,
    Always,
    Never,
}

/// `src` laid out at `cols` columns: styled (SGR, OSC 8 links) or plain.
pub fn render(src: &str, cols: usize, styled: bool) -> String {
    let lines = markdown::layout(src, cols);
    if styled { markdown::to_ansi(&lines, cols, true) } else { markdown::to_text(&lines, cols) }
}

/// Columns to lay out at: `--width`, else the terminal's, else `COLUMNS`, else 80.
pub fn columns(width: Option<usize>) -> usize {
    width.or_else(terminal_columns).or_else(|| std::env::var("COLUMNS").ok()?.parse().ok()).unwrap_or(80).clamp(20, 400)
}

/// Render `files` (`-`: stdin; none: stdin) one after another.
pub fn run(files: &[String], width: Option<usize>, color: Color, out: &mut dyn Write) -> Result<()> {
    let styled = match color {
        Color::Always => true,
        Color::Never => false,
        Color::Auto => std::io::stdout().is_terminal(),
    };
    let cols = columns(width);
    let stdin = ["-".to_string()];
    let files = if files.is_empty() { &stdin[..] } else { files };
    for (i, f) in files.iter().enumerate() {
        let src = if f == "-" {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).context("reading stdin")?;
            s
        } else {
            let b = std::fs::read(f).with_context(|| format!("reading {f}"))?;
            String::from_utf8_lossy(&b).into_owned()
        };
        if i > 0 {
            writeln!(out)?;
        }
        out.write_all(render(&src, cols, styled).as_bytes())?;
    }
    out.flush()?;
    Ok(())
}

/// The width of the terminal on stdout, if it is one.
#[cfg(unix)]
fn terminal_columns() -> Option<usize> {
    // SAFETY: TIOCGWINSZ fills a winsize; nothing else is touched.
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } == 0;
    (ok && ws.ws_col > 0).then_some(ws.ws_col as usize)
}

/// The width of the console on stdout, if it is one.
#[cfg(windows)]
fn terminal_columns() -> Option<usize> {
    use windows_sys::Win32::System::Console::{CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo, GetStdHandle, STD_OUTPUT_HANDLE};
    // SAFETY: the handle is the process's own stdout; info is plain data.
    unsafe {
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
        (GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &mut info) != 0)
            .then(|| (info.srWindow.Right - info.srWindow.Left + 1).max(0) as usize)
            .filter(|&c| c > 0)
    }
}

#[cfg(not(any(unix, windows)))]
fn terminal_columns() -> Option<usize> {
    None
}
