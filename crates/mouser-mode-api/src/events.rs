//! Events and listeners (EVENTS.md sections 3, 4, 8 and 11). Host events and
//! plugin-emitted events go through one dispatcher with the same matching,
//! permissions and ordering; a plugin hears its own emits unless a listener
//! opts out. The rules a dispatcher enforces live here so host and tests
//! share them.

use serde_json::Value;
use std::time::Instant;

/// Emits deeper than this in one cause chain are dropped.
pub const MAX_HOPS: u8 = 8;
/// One emit reaches at most this many listeners.
pub const MAX_FANOUT: usize = 64;
/// Sync listener timeout: default and the most a listener may ask for.
pub const SYNC_TIMEOUT_MS: u32 = 16;
pub const SYNC_TIMEOUT_MAX_MS: u32 = 50;
/// Bounded per-plugin outbound queue.
pub const QUEUE_LEN: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Origin {
    Host,
    Plugin { name: String, session: String },
    Mcp { client: String },
}

/// Every event, from the host or emitted by a plugin or MCP client.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Event {
    pub seq: u64,
    pub ts_us: u64,
    pub tab: Option<u32>,
    /// `shell.cwd_changed`, `input.key`, `commander.selection` ...
    pub name: String,
    pub payload: Value,
    pub origin: Origin,
    /// Seq of the event whose handler emitted this one.
    pub cause: Option<u64>,
    /// 0 for host input; +1 per emit made while handling an event.
    pub hops: u8,
    /// Emitted by a plugin or MCP client rather than observed by the host.
    pub synthetic: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Delivery {
    #[default]
    Async,
    /// May answer `Handled`/`Pass` within `timeout_ms` (≤ 50).
    Sync { timeout_ms: u32 },
    /// Answers `EmitRequest`s.
    Responder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Coalesce {
    Latest,
    Sum,
    Batch { max_ms: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Filter {
    /// Only while this plugin's mode is active.
    pub mode_active: bool,
    pub active_tab: bool,
    pub mime: Option<String>,
    /// Hear this plugin's own emits (default true).
    pub from_self: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Listener {
    /// Exact `shell.cwd_changed` or a trailing glob `input.*`.
    pub event: String,
    pub filter: Option<Filter>,
    pub delivery: Delivery,
    pub coalesce: Option<Coalesce>,
    pub max_rate_hz: Option<u16>,
}

impl Listener {
    pub fn on(event: &str) -> Self {
        Self { event: event.into(), filter: None, delivery: Delivery::Async, coalesce: None, max_rate_hz: None }
    }

    pub fn matches(&self, name: &str) -> bool {
        matches(&self.event, name)
    }

    pub fn hears_self(&self) -> bool {
        self.filter.as_ref().and_then(|f| f.from_self).unwrap_or(true)
    }
}

/// `pattern` is exact, `*`, or ends in `.*` (any name below that prefix).
pub fn matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some("") => true,
        Some(prefix) => prefix.ends_with('.') && name.starts_with(prefix),
        None => pattern == name,
    }
}

/// A custom event a plugin declares in `Hello.emits`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EventDecl {
    pub name: String,
    /// JSON schema of the payload, shown by MCP `events_list`.
    pub schema: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EmitResult {
    Accepted {
        delivered: u32,
    },
    Throttled,
    Denied(String),
    /// Over [`MAX_HOPS`].
    Loop,
}

/// Facts only the host can assert: never emitted by a plugin or MCP client.
pub const HOST_FACTS: &[&str] =
    &["shell.", "term.", "agent.", "approval.", "tab.", "plugin.", "models.", "frame.", "fs.", "proc.", "view.", "media."];

/// Input-style events a plugin may synthesize into its own mode.
pub const SELF_INPUT: &[&str] = &["input.key", "input.text", "ui.action", "input.scroll", "timer.fired"];

/// Whether `plugin` (which declared `declared`) may emit `name`. `into`
/// names the mode a synthetic input event targets (`None`: own mode);
/// `granted` holds `events.inject:<plugin>` grants.
pub fn may_emit(plugin: &str, declared: &[EventDecl], name: &str, into: Option<&str>, granted: &[String]) -> Result<(), String> {
    if HOST_FACTS.iter().any(|p| name.starts_with(p)) {
        return Err(format!("{name} is a host event; plugins can't emit it"));
    }
    if SELF_INPUT.contains(&name) {
        return match into {
            None => Ok(()),
            Some(t) if t == plugin => Ok(()),
            Some(t) if granted.iter().any(|g| *g == format!("events.inject:{t}")) => Ok(()),
            Some(t) => Err(format!("injecting {name} into {t} needs events.inject:{t}")),
        };
    }
    let own = name.strip_prefix(plugin).is_some_and(|r| r.starts_with('.'));
    if !own {
        return Err(format!("{name} is outside {plugin}'s namespace"));
    }
    if !declared.iter().any(|d| d.name == name) {
        return Err(format!("{name} was not declared in emits"));
    }
    Ok(())
}

/// Hop count for an emit made while handling `cause`, or `None` (a loop).
pub fn next_hops(cause_hops: u8) -> Option<u8> {
    let h = cause_hops.saturating_add(1);
    (h <= MAX_HOPS).then_some(h)
}

/// Emit rate per plugin: 200/s sustained, bursts of 1000.
#[derive(Debug, Clone)]
pub struct RateLimit {
    tokens: f64,
    rate: f64,
    burst: f64,
    last: Instant,
}

impl RateLimit {
    pub fn new(now: Instant) -> Self {
        Self::with(200.0, 1000.0, now)
    }

    pub fn with(rate: f64, burst: f64, now: Instant) -> Self {
        Self { tokens: burst, rate, burst, last: now }
    }

    /// Take a token; false: throttled.
    pub fn take(&mut self, now: Instant) -> bool {
        let dt = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + dt * self.rate).min(self.burst);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}
