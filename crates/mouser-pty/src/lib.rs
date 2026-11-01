use anyhow::Result;
pub use portable_pty;
pub use portable_pty::{Child, MasterPty};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::env;
use std::io::{Read, Write};
use std::path::Path;

pub struct PtyHandles {
    pub child: Box<dyn Child + Send>,
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub master: Box<dyn MasterPty + Send>,
}

pub fn spawn_shell(cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> Result<PtyHandles> {
    spawn_shell_in(None, cols, rows, pixel_width, pixel_height)
}

/// The login shell, started in `cwd` (default: `$HOME`).
pub fn spawn_shell_in(cwd: Option<&Path>, cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> Result<PtyHandles> {
    let mut cmd = shell_command();
    if let Some(d) = cwd {
        cmd.cwd(d);
    }
    spawn_in_pty(cmd, cols, rows, pixel_width, pixel_height)
}

/// Run `argv` directly as the PTY's process (no shell in between), with the
/// same terminal environment as the login shell.
pub fn spawn_argv(argv: &[String], cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> Result<PtyHandles> {
    spawn_argv_in(argv, None, cols, rows, pixel_width, pixel_height)
}

/// `spawn_argv` in `cwd` (default: `$HOME`).
pub fn spawn_argv_in(argv: &[String], cwd: Option<&Path>, cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> Result<PtyHandles> {
    let (prog, args) = argv.split_first().ok_or_else(|| anyhow::anyhow!("empty command"))?;
    let mut cmd = CommandBuilder::new(prog);
    cmd.args(args);
    if let Some(home) = env::var_os("HOME") {
        cmd.cwd(home);
    }
    if let Some(d) = cwd {
        cmd.cwd(d);
    }
    term_env(&mut cmd);
    spawn_in_pty(cmd, cols, rows, pixel_width, pixel_height)
}

fn term_env(cmd: &mut CommandBuilder) {
    cmd.env("TERM", term_name());
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "Mouser");
    cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
}

fn spawn_in_pty(cmd: CommandBuilder, cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> Result<PtyHandles> {
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize { rows, cols, pixel_width, pixel_height })?;
    let child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    let reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;
    let master = pair.master;
    Ok(PtyHandles { child, reader, writer, master })
}

pub fn set_winsize(handles: &mut PtyHandles, cols: u16, rows: u16) -> Result<()> {
    handles.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
    Ok(())
}

/// TIOCSWINSZ with the grid's pixel size, so `ws_xpixel`/`ws_ypixel` are real.
pub fn set_master_winsize(master: &mut (dyn MasterPty + Send), cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> Result<()> {
    master.resize(PtySize { rows, cols, pixel_width, pixel_height })?;
    Ok(())
}

/// The user's login shell, started as a login shell in their home directory.
pub fn shell_command() -> CommandBuilder {
    let shell = env::var("SHELL").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(default_shell);
    let mut cmd = CommandBuilder::new(&shell);
    #[cfg(unix)]
    cmd.arg("-l");
    if let Some(home) = env::var_os("HOME") {
        cmd.cwd(home);
    }
    term_env(&mut cmd);
    cmd
}

fn default_shell() -> String {
    if cfg!(windows) { "powershell.exe".to_string() } else { "/bin/sh".to_string() }
}

/// Mouser's own `mouser` entry when installed (`extra/mouser.terminfo`,
/// `tic -x`), otherwise `xterm-256color`. Never `xterm-kitty`: Mouser only
/// implements Kitty's keyboard protocol, not the rest of Kitty.
pub fn term_name() -> &'static str {
    if terminfo_exists("mouser") { "mouser" } else { "xterm-256color" }
}

fn terminfo_exists(name: &str) -> bool {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Some(d) = env::var_os("TERMINFO") {
        dirs.push(d.into());
    }
    if let Some(home) = env::var_os("HOME") {
        dirs.push(std::path::Path::new(&home).join(".terminfo"));
    }
    if let Ok(list) = env::var("TERMINFO_DIRS") {
        dirs.extend(list.split(':').filter(|d| !d.is_empty()).map(Into::into));
    }
    for d in ["/etc/terminfo", "/lib/terminfo", "/usr/share/terminfo", "/usr/lib/terminfo", "/usr/share/lib/terminfo"] {
        dirs.push(d.into());
    }
    let first = name.chars().next().unwrap_or('x');
    let hex = format!("{:x}", first as u32);
    dirs.iter().any(|d| d.join(first.to_string()).join(name).exists() || d.join(&hex).join(name).exists())
}
