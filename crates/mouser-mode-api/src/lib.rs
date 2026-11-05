//! The public API every Mouser mode plugin is built on (docs/protocol.md).
//!
//! A mode is its own executable. Mouser starts it once, keeps it running and
//! talks to it over its stdin/stdout with length-prefixed frames
//! ([`codec`]), MessagePack by default or JSON with `MOUSER_MODE_WIRE=json`.
//! The first exchange is a handshake ([`protocol::Hello`] /
//! [`protocol::Welcome`]) that picks a shared protocol version
//! ([`protocol::negotiate`]) and registers the plugin's event listeners.
//! Then events flow in ([`events`]) and views, effects, emits and host calls
//! ([`host_api`]) flow out. Images travel through a shared-memory ring
//! ([`shm`]) with inline bytes as the fallback. The host restarts a crashed
//! plugin with back-off and gives up after a cap ([`supervisor`]). Plugins
//! reach host features (remote files, preferences, models, permissions,
//! fetch) through the [`services`] traits, answered over RPC.
//!
//! Commander, chat and preferences are built on this crate exactly like a
//! third-party mode.

pub mod codec;
pub mod events;
#[cfg(feature = "markdown")]
pub mod highlight;
pub mod host;
pub mod host_api;
#[cfg(feature = "markdown")]
pub mod markdown;
pub mod mode;
pub mod protocol;
pub mod serve;
pub mod services;
pub mod shm;
pub mod supervisor;
pub mod ui;

pub use mode::{Context, Effect};
pub use protocol::{API_VERSION, ApiRange};
pub use serve::{Host, Mode, serve};
pub use ui::{Action, Panel, PanelItem, PanelStyle, Rect, Viewport};
