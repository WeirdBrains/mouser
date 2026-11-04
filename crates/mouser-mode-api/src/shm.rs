//! A ring of image slots in shared memory. The host creates it and passes
//! its path in the handshake; the plugin maps the same file, writes RGBA
//! into a slot and sends only an [`crate::protocol::ImageRef`]. On Linux the
//! file lives in `/dev/shm` (memory only); elsewhere in the temp directory,
//! which the OS keeps in its page cache. Where neither can be mapped (some
//! sandboxes) the host passes no ring and images go inline.

use memmap2::MmapMut;
use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};

pub struct Ring {
    map: MmapMut,
    path: PathBuf,
    slots: u32,
    slot_bytes: usize,
    owner: bool,
}

fn dir() -> PathBuf {
    let d = Path::new("/dev/shm");
    if d.is_dir() { d.into() } else { std::env::temp_dir() }
}

impl Ring {
    /// Host side: a fresh ring named after `tag` (removed on drop).
    pub fn create(tag: &str, slots: u32, slot_bytes: usize) -> io::Result<Ring> {
        let path = dir().join(format!("mouser-mode-{tag}-{}", std::process::id()));
        let f = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path)?;
        f.set_len(slots as u64 * slot_bytes as u64)?;
        // SAFETY: the file is ours; the plugin only writes whole slots the
        // host is not reading (double buffering, see docs/protocol.md).
        let map = unsafe { MmapMut::map_mut(&f)? };
        Ok(Ring { map, path, slots, slot_bytes, owner: true })
    }

    /// Plugin side: map the ring from the handshake.
    pub fn open(spec: &crate::protocol::ShmSpec) -> io::Result<Ring> {
        let f = OpenOptions::new().read(true).write(true).open(&spec.path)?;
        let need = spec.slots as u64 * spec.slot_bytes;
        if f.metadata()?.len() < need {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "shared-memory ring is smaller than announced"));
        }
        // SAFETY: as in `create`.
        let map = unsafe { MmapMut::map_mut(&f)? };
        Ok(Ring { map, path: spec.path.clone().into(), slots: spec.slots, slot_bytes: spec.slot_bytes as usize, owner: false })
    }

    pub fn spec(&self) -> crate::protocol::ShmSpec {
        crate::protocol::ShmSpec { path: self.path.display().to_string(), slots: self.slots, slot_bytes: self.slot_bytes as u64 }
    }

    pub fn slots(&self) -> u32 {
        self.slots
    }

    pub fn slot(&self, i: u32) -> Option<&[u8]> {
        (i < self.slots).then(|| &self.map[i as usize * self.slot_bytes..(i as usize + 1) * self.slot_bytes])
    }

    pub fn slot_mut(&mut self, i: u32) -> Option<&mut [u8]> {
        let n = self.slot_bytes;
        (i < self.slots).then(|| &mut self.map[i as usize * n..(i as usize + 1) * n])
    }
}

impl Drop for Ring {
    fn drop(&mut self) {
        if self.owner {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
