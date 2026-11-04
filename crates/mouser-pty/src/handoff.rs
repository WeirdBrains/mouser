//! Default-terminal handoff plumbing (docs/windows.md §Default terminal).
//!
//! When Mouser is the Windows default terminal, the console host (Windows
//! Terminal's OpenConsole) creates a pseudoconsole for a console program and
//! hands Mouser the ends of its pipes over COM (`ITerminalHandoff3`). This
//! module turns those pipes into the same [`PtyHandles`] a spawned shell has,
//! so a handed-off session is just another tab. The pieces here are plain
//! `Read`/`Write` plumbing and are tested on every OS; the COM server itself
//! is in `handoff_win`.

use crate::PtyHandles;
use anyhow::Result;
use portable_pty::{Child, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::Mutex;

/// ConPTY's out-of-band signal for "the window is now `cols` x `rows`"
/// (`PTY_SIGNAL_RESIZE_WINDOW`).
pub const PTY_SIGNAL_RESIZE_WINDOW: u16 = 8;

/// The 6-byte resize packet written to the signal pipe: code, width, height
/// as little-endian `u16`s (ConPTY's `RESIZE_PSEUDO_CONSOLE_PACKET`).
pub fn resize_packet(cols: u16, rows: u16) -> [u8; 6] {
    let (c, w, h) = (PTY_SIGNAL_RESIZE_WINDOW.to_le_bytes(), cols.max(1).to_le_bytes(), rows.max(1).to_le_bytes());
    [c[0], c[1], w[0], w[1], h[0], h[1]]
}

/// Whether this process was started by COM to serve a class object
/// (COM appends `-Embedding` or `/Embedding` to the registered command line).
pub fn is_com_embedding<S: AsRef<str>>(args: &[S]) -> bool {
    args.iter().skip(1).any(|a| a.as_ref().eq_ignore_ascii_case("-embedding") || a.as_ref().eq_ignore_ascii_case("/embedding"))
}

/// A pseudoconsole that someone else created: output and input pipes plus
/// the signal pipe that carries resizes.
pub struct PipeMaster {
    reader: Mutex<Option<Box<dyn Read + Send>>>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    signal: Mutex<Box<dyn Write + Send>>,
    size: Mutex<PtySize>,
}

impl PipeMaster {
    pub fn new(reader: Box<dyn Read + Send>, writer: Box<dyn Write + Send>, signal: Box<dyn Write + Send>) -> Self {
        Self {
            reader: Mutex::new(Some(reader)),
            writer: Mutex::new(Some(writer)),
            signal: Mutex::new(signal),
            size: Mutex::new(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }),
        }
    }
}

impl MasterPty for PipeMaster {
    fn resize(&self, size: PtySize) -> Result<()> {
        let mut cur = self.size.lock().unwrap();
        if (cur.cols, cur.rows) != (size.cols, size.rows) || cur.pixel_width == u16::MAX {
            let mut s = self.signal.lock().unwrap();
            s.write_all(&resize_packet(size.cols, size.rows))?;
            s.flush()?;
        }
        *cur = size;
        Ok(())
    }

    fn get_size(&self) -> Result<PtySize> {
        Ok(*self.size.lock().unwrap())
    }

    fn try_clone_reader(&self) -> Result<Box<dyn Read + Send>> {
        self.reader.lock().unwrap().take().ok_or_else(|| anyhow::anyhow!("handoff output pipe already taken"))
    }

    fn take_writer(&self) -> Result<Box<dyn Write + Send>> {
        self.writer.lock().unwrap().take().ok_or_else(|| anyhow::anyhow!("handoff input pipe already taken"))
    }

    #[cfg(unix)]
    fn process_group_leader(&self) -> Option<libc_pid::pid_t> {
        None
    }

    #[cfg(unix)]
    fn as_raw_fd(&self) -> Option<std::os::unix::io::RawFd> {
        None
    }
}

#[cfg(unix)]
mod libc_pid {
    #[allow(non_camel_case_types)]
    pub type pid_t = i32;
}

/// What a handoff delivered, ready to become a tab.
pub struct Handoff {
    /// Title from the shortcut or `STARTUPINFO` (may be empty).
    pub title: String,
    pub handles: PtyHandles,
}

/// Wrap handed-off pipes as [`PtyHandles`]. The first resize always goes out
/// so the console learns the tab's real size.
pub fn handles_from_pipes(
    output: Box<dyn Read + Send>,
    input: Box<dyn Write + Send>,
    signal: Box<dyn Write + Send>,
    child: Box<dyn Child + Send>,
) -> Result<PtyHandles> {
    let master = PipeMaster::new(output, input, signal);
    // Force the first real resize to be sent even if it matches the default.
    master.size.lock().unwrap().pixel_width = u16::MAX;
    let reader = master.try_clone_reader()?;
    let writer = master.take_writer()?;
    Ok(PtyHandles { child, reader, writer, master: Box::new(master) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use portable_pty::{ChildKiller, ExitStatus};
    use std::sync::Arc;

    #[derive(Debug)]
    struct FakeChild;
    impl ChildKiller for FakeChild {
        fn kill(&mut self) -> std::io::Result<()> {
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(FakeChild)
        }
    }
    impl Child for FakeChild {
        fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
            Ok(Some(ExitStatus::with_exit_code(3)))
        }
        fn wait(&mut self) -> std::io::Result<ExitStatus> {
            Ok(ExitStatus::with_exit_code(3))
        }
        fn process_id(&self) -> Option<u32> {
            Some(42)
        }
        #[cfg(windows)]
        fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
            None
        }
    }

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl Write for Sink {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn resize_packet_layout() {
        assert_eq!(resize_packet(120, 40), [8, 0, 120, 0, 40, 0]);
        assert_eq!(resize_packet(0, 0), [8, 0, 1, 0, 1, 0]);
        assert_eq!(resize_packet(300, 2), [8, 0, 0x2c, 1, 2, 0]);
    }

    #[test]
    fn embedding_flag() {
        assert!(is_com_embedding(&["mouser.exe", "-Embedding"]));
        assert!(is_com_embedding(&["mouser.exe", "/embedding"]));
        assert!(!is_com_embedding(&["-Embedding"]));
        assert!(!is_com_embedding(&["mouser.exe", "-e", "cmd"]));
    }

    #[test]
    fn pipes_carry_output_input_and_resizes() {
        let (input, signal) = (Sink::default(), Sink::default());
        let out: Box<dyn Read + Send> = Box::new(std::io::Cursor::new(b"hello from conpty".to_vec()));
        let mut h = handles_from_pipes(out, Box::new(input.clone()), Box::new(signal.clone()), Box::new(FakeChild)).unwrap();
        let mut got = String::new();
        h.reader.read_to_string(&mut got).unwrap();
        assert_eq!(got, "hello from conpty");
        h.writer.write_all(b"dir\r").unwrap();
        assert_eq!(&*input.0.lock().unwrap(), b"dir\r");
        // The first resize is sent even at the default size; repeats are not.
        let sz = |cols, rows| PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
        h.master.resize(sz(80, 24)).unwrap();
        h.master.resize(sz(80, 24)).unwrap();
        h.master.resize(sz(100, 30)).unwrap();
        assert_eq!(&*signal.0.lock().unwrap(), &[8, 0, 80, 0, 24, 0, 8, 0, 100, 0, 30, 0]);
        assert_eq!((h.master.get_size().unwrap().cols, h.master.get_size().unwrap().rows), (100, 30));
        assert!(h.master.try_clone_reader().is_err());
        assert!(h.master.take_writer().is_err());
        assert_eq!(h.child.wait().unwrap().exit_code(), 3);
    }

    #[cfg(unix)]
    #[test]
    fn real_os_pipes() {
        let (r_out, mut w_out) = std::io::pipe().unwrap();
        let (mut r_in, w_in) = std::io::pipe().unwrap();
        let mut h = handles_from_pipes(Box::new(r_out), Box::new(w_in), Box::new(Sink::default()), Box::new(FakeChild)).unwrap();
        w_out.write_all(b"PS C:\\> ").unwrap();
        drop(w_out);
        let mut got = Vec::new();
        h.reader.read_to_end(&mut got).unwrap();
        assert_eq!(got, b"PS C:\\> ");
        h.writer.write_all(b"exit\r").unwrap();
        drop(h.writer);
        let mut sent = Vec::new();
        r_in.read_to_end(&mut sent).unwrap();
        assert_eq!(sent, b"exit\r");
    }
}
