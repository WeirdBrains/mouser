//! Host services a plugin may call. The host implements these traits and
//! answers [`crate::host_api::Call::Service`] requests with them, checking the
//! permission named on each method first; the plugin SDK offers the same
//! methods as RPC clients. Model calls go through [`AgentService`], so a
//! plugin never sees an API key.

use serde_json::Value;

pub type Result<T> = std::result::Result<T, String>;

/// Files on the shell's remote host (`remote.read`).
pub trait RemoteService {
    /// `user@host` and directory of the shell, if it is remote.
    fn location(&self) -> Option<(String, String)>;
    /// List a remote directory as `mouser_core::commander::Entry` values.
    fn list(&self, dir: &str) -> Result<Value>;
}

/// Preferences (`prefs.write` for `set`): the host validates values and
/// writes the file keeping its comments.
pub trait PrefsService {
    fn schema(&self) -> Value;
    fn get(&self, key: &str) -> Result<Value>;
    fn set(&self, key: &str, value: Value) -> Result<()>;
}

/// Models and the agent (`agent.use`). Tools run in the host after a
/// host-drawn approval; replies stream back as `agent.stream` chunks.
pub trait AgentService {
    fn models(&self) -> Result<Value>;
    /// Start a turn; returns a stream id.
    fn send(&self, conversation: &str, text: &str) -> Result<u64>;
    fn cancel(&self, stream: u64);
}

/// What this plugin was granted, and asking for more (host-drawn popup).
pub trait PermissionService {
    fn granted(&self) -> Vec<String>;
    fn request(&self, permission: &str, why: &str) -> Result<bool>;
}

/// HTTP(S) fetches made by the host (`net.fetch`).
pub trait FetchService {
    fn fetch(&self, url: &str) -> Result<Vec<u8>>;
}

/// Method names and the permission each needs.
pub const METHODS: &[(&str, Option<&str>)] = &[
    ("remote.location", Some("remote.read")),
    ("remote.list", Some("remote.read")),
    ("remote.read", Some("remote.read")),
    ("prefs.schema", None),
    ("prefs.get", None),
    ("prefs.set", Some("prefs.write")),
    ("prefs.reset", Some("prefs.write")),
    ("prefs.secret_prompt", Some("prefs.write")),
    ("agent.models", Some("agent.use")),
    ("agent.set_default", Some("agent.use")),
    ("agent.send", Some("agent.use")),
    ("agent.cancel", Some("agent.use")),
    ("agent.clear", Some("agent.use")),
    ("agent.state", Some("agent.use")),
    ("permissions.granted", None),
    ("permissions.request", None),
    ("fetch", Some("net.fetch")),
];

/// Whether `granted` allows calling `method`; unknown methods are refused.
pub fn allowed(method: &str, granted: &[String]) -> bool {
    match METHODS.iter().find(|(m, _)| *m == method) {
        Some((_, None)) => true,
        Some((_, Some(p))) => granted.iter().any(|g| g == p),
        None => false,
    }
}
