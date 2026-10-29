use anyhow::Result;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::env;
use std::io::{Read, Write};

pub struct PtyHandles {
    pub child: Box<dyn Child + Send>,
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub master: Box<dyn MasterPty + Send>,
}

pub fn spawn_shell(cols: u16, rows: u16) -> Result<PtyHandles> {
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;

    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let mut cmd = CommandBuilder::new(shell);
    cmd.env("TERM", "xterm-kitty");
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

pub fn set_master_winsize(master: &mut (dyn MasterPty + Send), cols: u16, rows: u16) -> Result<()> {
    master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
    Ok(())
}

