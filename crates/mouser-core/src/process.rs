//! Spawning helper programs without console windows.
//!
//! On Windows Mouser is a GUI-subsystem program with no console, so every
//! console child (ssh, ffmpeg, plugins…) would otherwise get a console of its
//! own: a conhost window or, when Windows Terminal is the default terminal
//! application, a Windows Terminal window whose closing kills the child.

use std::ffi::OsStr;
use std::process::Command;

/// `Command::new(program)` that opens no console window on Windows.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}
