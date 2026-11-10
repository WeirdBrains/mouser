//! Framing: a 4-byte little-endian length, then one message in MessagePack
//! (default) or JSON (`MOUSER_MODE_WIRE=json`, for authors in any language).

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io::{self, Read, Write};

/// Larger frames are refused (an inline 4K RGBA frame is ~33 MB).
pub const MAX_FRAME: usize = 64 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Wire {
    #[default]
    MsgPack,
    Json,
}

impl Wire {
    /// From `MOUSER_MODE_WIRE` (`json` or `msgpack`).
    pub fn from_env() -> Wire {
        match std::env::var("MOUSER_MODE_WIRE").as_deref() {
            Ok("json") => Wire::Json,
            _ => Wire::MsgPack,
        }
    }
}

fn bad(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

/// Write one message, MessagePack structs as arrays (the plugin side: the
/// host reads both shapes).
pub fn write<T: Serialize>(w: &mut impl Write, wire: Wire, msg: &T) -> io::Result<()> {
    let body = match wire {
        Wire::MsgPack => rmp_serde::to_vec(msg).map_err(bad)?,
        Wire::Json => serde_json::to_vec(msg).map_err(bad)?,
    };
    frame(w, body)
}

/// Write one message with MessagePack structs as maps keyed by field name
/// (what the host sends a plugin). A plugin built before a field was added
/// skips the unknown key; as an array the extra element made its decoder
/// fail ("array had incorrect length") and the plugin exit at the
/// handshake. Plugins built from any mode-api version read both shapes.
pub fn write_named<T: Serialize>(w: &mut impl Write, wire: Wire, msg: &T) -> io::Result<()> {
    let body = match wire {
        Wire::MsgPack => rmp_serde::to_vec_named(msg).map_err(bad)?,
        Wire::Json => serde_json::to_vec(msg).map_err(bad)?,
    };
    frame(w, body)
}

fn frame(w: &mut impl Write, body: Vec<u8>) -> io::Result<()> {
    if body.len() > MAX_FRAME {
        return Err(bad(format!("frame of {} bytes is over the {MAX_FRAME} limit", body.len())));
    }
    w.write_all(&(body.len() as u32).to_le_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

/// Read one message; `Ok(None)` at a clean end of stream. `buf` is reused.
pub fn read<T: DeserializeOwned>(r: &mut impl Read, wire: Wire, buf: &mut Vec<u8>) -> io::Result<Option<T>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let n = u32::from_le_bytes(len) as usize;
    if n > MAX_FRAME {
        return Err(bad(format!("frame of {n} bytes is over the {MAX_FRAME} limit")));
    }
    buf.resize(n, 0);
    r.read_exact(buf)?;
    Ok(Some(match wire {
        Wire::MsgPack => rmp_serde::from_slice(buf).map_err(bad)?,
        Wire::Json => serde_json::from_slice(buf).map_err(bad)?,
    }))
}
