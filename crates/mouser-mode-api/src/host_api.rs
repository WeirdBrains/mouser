//! Calls a plugin makes to the host (EVENTS.md sections 5 and 13). Every
//! call runs with the logged-in user's rights (or the SSH login's on a
//! remote target) and never more: elevation is only `Escalate`, typed into
//! the user's visible terminal after a host-drawn card, so the OS or sudo
//! asks the user directly and Mouser never sees a password.

use serde_json::Value;

/// Where a call runs.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Target {
    /// The active tab's session (local or remote).
    #[default]
    Active,
    Local,
    /// A remote tab, over its existing SSH connection.
    Tab(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WriteMode {
    Create,
    Truncate,
    CreateNew,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Fs {
    Read {
        path: String,
        offset: u64,
        len: Option<u64>,
    },
    Write {
        path: String,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
        mode: WriteMode,
        perm: Option<u32>,
        atomic: bool,
    },
    Append {
        path: String,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    Stat {
        path: String,
        follow: bool,
    },
    List {
        path: String,
        glob: Option<String>,
        hidden: bool,
        cursor: Option<String>,
    },
    Mkdir {
        path: String,
        parents: bool,
        perm: Option<u32>,
    },
    /// `to_trash` defaults to true; permanent delete always shows a card.
    Remove {
        path: String,
        recursive: bool,
        to_trash: bool,
    },
    Rename {
        from: String,
        to: String,
        overwrite: bool,
    },
    Chmod {
        path: String,
        mode: u32,
    },
    Watch {
        path: String,
        recursive: bool,
    },
    Unwatch {
        id: u64,
    },
}

impl Fs {
    /// Whether the call changes files (always audited; cards outside scope).
    pub fn mutates(&self) -> bool {
        !matches!(self, Fs::Read { .. } | Fs::Stat { .. } | Fs::List { .. } | Fs::Watch { .. } | Fs::Unwatch { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Io {
    Pipes,
    Pty {
        cols: u16,
        rows: u16,
    },
    /// Typed into a real tab the user sees (`new`: a new tab).
    VisiblePty {
        new: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Spawn {
    pub program: String,
    pub args: Vec<String>,
    pub env_inherit: bool,
    pub env_set: Vec<(String, String)>,
    pub cwd: Option<String>,
    pub io: Io,
    pub timeout_ms: Option<u64>,
}

/// Programs that switch user: only allowed through `Io::VisiblePty`.
pub const ESCALATORS: &[&str] = &["sudo", "doas", "pkexec", "su", "run0"];

impl Spawn {
    /// Refuse privilege escalation on the plugin's private pipes or pty.
    pub fn check_escalation(&self) -> Result<(), String> {
        let base = self.program.rsplit(['/', '\\']).next().unwrap_or(&self.program);
        let base = base.strip_suffix(".exe").unwrap_or(base);
        if ESCALATORS.contains(&base) && !matches!(self.io, Io::VisiblePty { .. }) {
            return Err(format!("{base} must run in the visible terminal (EscalationNeedsVisiblePty)"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Signal {
    Term,
    Kill,
    Int,
}

/// A host call (`Out::Request`), answered by `In::Response`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Call {
    Fs {
        target: Target,
        op: Fs,
    },
    Spawn {
        target: Target,
        spec: Spawn,
    },
    Stdin {
        proc: u64,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
        close: bool,
    },
    Kill {
        proc: u64,
        signal: Signal,
    },
    Resize {
        proc: u64,
        cols: u16,
        rows: u16,
    },
    /// Run one command elevated: a card every time (no "Always"), then sudo
    /// in the visible terminal, UAC on Windows, the OS dialog on macOS.
    Escalate {
        target: Target,
        cmdline: String,
    },
    /// A host service method by name (`prefs.get`, `agent.send` ...).
    Service {
        method: String,
        params: Value,
    },
    /// Host-drawn approval popup; the answer comes as `approval.result`.
    Approve {
        title: String,
        detail: String,
        options: Vec<String>,
    },
    TimerSet {
        id: String,
        every_ms: Option<u64>,
        at_ms: Option<u64>,
    },
    TimerClear {
        id: String,
    },
    ClipboardGet,
    ClipboardSet(String),
}

/// Paths no plugin reads or writes without the user's own grant.
pub const DENY_LIST: &[&str] =
    &["$HOME/.ssh/**", "$HOME/.gnupg/**", "$MOUSER_HOME/secrets/**", "$MOUSER_HOME/config.toml", "$HOME/Library/Keychains/**"];
