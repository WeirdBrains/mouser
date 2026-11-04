//! Wire messages and version negotiation (docs/protocol.md).

use crate::mode::{Context, Effect};
use crate::ui::Rect;

/// The newest protocol version this crate speaks. `0` while the protocol is
/// still changing; frozen to `1` once the bundled modes run on it.
pub const API_VERSION: u32 = 0;
/// The oldest version a host built from this crate still accepts: the
/// current and the previous major.
pub const API_MIN: u32 = API_VERSION.saturating_sub(1);

/// An inclusive range of protocol versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ApiRange {
    pub min: u32,
    pub max: u32,
}

impl ApiRange {
    pub const CURRENT: ApiRange = ApiRange { min: API_MIN, max: API_VERSION };
}

/// The newest version both sides speak, or why there is none.
pub fn negotiate(host: ApiRange, plugin: ApiRange) -> Result<u32, String> {
    let lo = host.min.max(plugin.min);
    let hi = host.max.min(plugin.max);
    if lo <= hi && plugin.min <= plugin.max {
        Ok(hi)
    } else if plugin.max < host.min {
        Err(format!("plugin speaks api {}-{}, Mouser needs {}-{}: update the plugin", plugin.min, plugin.max, host.min, host.max))
    } else {
        Err(format!("plugin speaks api {}-{}, Mouser speaks {}-{}: update Mouser", plugin.min, plugin.max, host.min, host.max))
    }
}

/// The plugin's first message.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Hello {
    pub api: ApiRange,
    pub name: String,
    pub version: String,
    pub hotkey: char,
    /// (action id, default key, description).
    pub bindings: Vec<(String, String, String)>,
    pub permissions: Vec<String>,
    /// The plugin can write images into a shared-memory ring.
    pub shm: bool,
}

/// The host's answer to [`Hello`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Welcome {
    Accepted {
        api: u32,
        ctx: Context,
        /// Permissions the user (or the installer's pre-grant) allowed.
        granted: Vec<String>,
        /// Path of the shared-memory ring to map, `None`: send images inline.
        shm: Option<ShmSpec>,
    },
    /// No shared version (or the plugin is disabled): the plugin should exit.
    Rejected { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShmSpec {
    pub path: String,
    pub slots: u32,
    pub slot_bytes: u64,
}

/// Host to mode.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum HostMsg {
    Activate,
    Restore {
        cwd: Option<String>,
    },
    Deactivate,
    Action {
        id: String,
        arg: i32,
    },
    Text(#[serde(with = "serde_bytes")] Vec<u8>),
    Escape,
    CwdChanged(String),
    Resize(Context),
    ThemeChanged,
    /// Sent only to modes that asked for ticks (the player).
    Tick {
        seq: u64,
    },
    /// Answer to a [`ModeMsg::Request`].
    Reply {
        id: u64,
        result: Result<serde_json::Value, String>,
    },
    Shutdown,
}

/// A picture in the view: in shared-memory slot `slot`, or inline.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImageRef {
    /// Cache key: the host uploads a key once and reuses the texture.
    pub key: u64,
    pub width: u32,
    pub height: u32,
    pub src: (u32, u32, u32, u32),
    pub rect: Rect,
    pub pixels: Pixels,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Pixels {
    /// Already uploaded under this key.
    Cached,
    Slot(u32),
    Inline(#[serde(with = "serde_bytes")] Vec<u8>),
}

/// Mode to host.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ModeMsg {
    Hello(Hello),
    /// The view changed. Panels are carried as the host's JSON panel model
    /// until the panel types move into this crate.
    View {
        dock: Option<serde_json::Value>,
        panel: Option<serde_json::Value>,
        images: Vec<ImageRef>,
        fullscreen: bool,
    },
    Effect(Effect),
    /// Something changed off the event path (a download finished).
    Wake,
    Log(String),
    /// Call a host service ([`crate::services`]) by method name.
    Request {
        id: u64,
        method: String,
        params: serde_json::Value,
    },
    /// Ask for (or stop) `Tick`s at this rate.
    Ticks {
        fps: u32,
    },
}
