//! Wire messages and version negotiation (docs/mode-protocol.md). The shape
//! follows the approved event spec: a `Hello` that registers listeners and
//! declares emitted events, a `Welcome` that grants or denies them, events
//! in ([`crate::events::Event`]) and [`Out`] messages back.

use crate::events::{EventDecl, Listener};
use crate::host_api::Call;
use crate::mode::{Context, Effect};
use crate::ui::{Panel, Rect};
use serde_json::Value;

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

/// A key binding a mode declares: action id, default key, description.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BindingDecl {
    pub id: String,
    pub key: String,
    pub describe: String,
}

/// What a plugin provides.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Provide {
    Mode {
        hotkey: char,
        bindings: Vec<BindingDecl>,
        /// Events that may activate this mode (`commander.ask_chat` for chat).
        activate_on: Vec<String>,
    },
    /// A background service with no view.
    Service,
}

/// The plugin's first message.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Hello {
    pub api: ApiRange,
    /// Must match `plugin.toml`.
    pub name: String,
    pub version: String,
    pub provides: Vec<Provide>,
    pub listeners: Vec<Listener>,
    /// Custom events it may emit, all in its own `<name>.*` namespace.
    pub emits: Vec<EventDecl>,
    pub wants_mcp: bool,
    /// The plugin can map a shared-memory ring.
    pub shm: bool,
}

/// The plugin's private MCP connection (scoped token, revoked when it stops).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct McpGrant {
    /// A Unix socket path or `\\.\pipe\mouser-mcp-<session>`.
    pub endpoint: String,
    pub token: String,
    /// `plugin:<name>`; approvals are keyed by it.
    pub client: String,
    pub tools: Vec<String>,
}

/// What Mouser tells an accepted plugin.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Accepted {
    pub api: u32,
    pub session: String,
    pub ctx: Context,
    /// Permissions granted (manifest, pre-grant or the user).
    pub granted: Vec<String>,
    /// Listeners refused, with the reason (not fatal).
    pub denied: Vec<(String, String)>,
    /// `None`: send images inline.
    pub shm: Option<ShmSpec>,
    pub mcp: Option<McpGrant>,
    /// Last [`Out::StateSave`] blob when restarted after a crash.
    pub restore: Option<Vec<u8>>,
}

/// The host's answer to [`Hello`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Welcome {
    Accepted(Box<Accepted>),
    /// No shared version (or the plugin is disabled): the plugin should exit.
    Rejected {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShmSpec {
    pub path: String,
    pub slots: u32,
    pub slot_bytes: u64,
}

/// Host to plugin, after the handshake.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum In {
    Event(crate::events::Event),
    /// Answer to an [`Out::Request`].
    Response {
        id: u64,
        result: Result<Value, String>,
    },
    EffectResult {
        ok: bool,
        refused: Option<String>,
    },
    EmitResult(crate::events::EmitResult),
    /// Answers to an [`Out::EmitRequest`].
    EmitResponses {
        id: u64,
        results: Vec<(String, Value)>,
        timed_out: Vec<String>,
    },
    /// 500 ms to save state and exit.
    Stopping {
        reason: String,
    },
}

/// A picture in the view.
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
    /// In this shared-memory slot.
    Slot(u32),
    /// Inline bytes (the Fuzzball graphics model, binary framed): when there
    /// is no ring.
    Inline {
        format: ImageFormat,
        #[serde(with = "serde_bytes")]
        bytes: Vec<u8>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ImageFormat {
    Rgba,
    Rgb,
    Png,
}

/// The answer of a sync listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Handled {
    Handled,
    Pass,
}

/// What the mode takes from the keyboard ([`Out::Wants`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Wants {
    /// Plain typing comes as `input.text`. Alt keys still go to the bindings
    /// (unless listed in `keys`) and the rest to the shell.
    pub text: bool,
    /// Escape sequences taken as `input.text` while `text` is on (arrows for
    /// a list, say).
    pub keys: Vec<String>,
    /// Something is open that Escape closes: Escape goes to the mode
    /// (`input.escape`) instead of to Mouser.
    pub escape: bool,
}

/// Plugin to host.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Out {
    Hello(Hello),
    /// The view changed (latest wins).
    View {
        dock: Option<Panel>,
        panel: Option<Panel>,
        images: Vec<ImageRef>,
        fullscreen: bool,
    },
    Effect(Effect),
    /// Which keys the mode takes now (latest wins). Without one, Mouser
    /// guesses from the view: a focused `Field` or an open `Popup` takes
    /// every key. Once sent, only this decides.
    Wants(Wants),
    /// A video frame already in shm `slot`; the host answers `frame.presented`.
    Frame {
        slot: u32,
        key: u64,
        width: u32,
        height: u32,
        pts_us: u64,
    },
    Emit {
        name: String,
        payload: Value,
    },
    EmitRequest {
        id: u64,
        name: String,
        payload: Value,
        timeout_ms: u32,
        all: bool,
    },
    /// Answer an `EmitRequest` delivered to a responder listener.
    Respond {
        seq: u64,
        value: Value,
    },
    Request {
        id: u64,
        call: Call,
    },
    /// For sync listeners.
    Reply {
        seq: u64,
        result: Handled,
    },
    StateSave(#[serde(with = "serde_bytes")] Vec<u8>),
    Subscribe(Listener),
    Unsubscribe(String),
    Log {
        level: LogLevel,
        text: String,
    },
    Wake,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}
