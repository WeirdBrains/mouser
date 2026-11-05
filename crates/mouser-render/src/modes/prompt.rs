//! Questions a mode plugin asks that Mouser itself must draw and answer
//! (docs/mode-protocol.md, "Mouser-drawn popups"): an `Approve` call, the
//! API-key prompt behind `prefs.secret_prompt`, and the agent's tool-call
//! cards. The adapter ([`super::stdio`]) hands them to the app through
//! [`super::Mode::take_prompts`]; the app shows them in its approval popup,
//! which owns the keyboard and mouse while open, so neither the plugin nor
//! the program in the tab can draw or answer it. A typed key goes from the
//! popup straight to the keychain: the plugin only hears "saved".

use std::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    /// `Call::Approve`: the plugin's own question, answered Once or Deny.
    Approve { title: String, detail: Vec<String> },
    /// `prefs.secret_prompt`: type a secret preference's value (masked).
    Secret { key: String, label: String },
    /// `permissions.request`: grant a permission the manifest declares
    /// (saved like any approval: once, always, deny, never).
    Permission { cap: String, detail: Vec<String> },
    /// A tool call the plugin's agent session wants to make.
    AgentTool { display: String, lines: Vec<String>, for_chat: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptAnswer {
    Once,
    /// "Allow for this chat" (agent tool cards only).
    ForChat,
    Deny,
    /// The typed value of a [`PromptKind::Secret`].
    Secret(String),
}

pub struct Prompt {
    /// The asking plugin's name, shown in the popup.
    pub plugin: String,
    pub kind: PromptKind,
    pub reply: mpsc::Sender<PromptAnswer>,
}

impl std::fmt::Debug for Prompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prompt").field("plugin", &self.plugin).field("kind", &self.kind).finish()
    }
}

impl Prompt {
    pub fn new(plugin: &str, kind: PromptKind) -> (Self, mpsc::Receiver<PromptAnswer>) {
        let (tx, rx) = mpsc::channel();
        (Prompt { plugin: plugin.into(), kind, reply: tx }, rx)
    }
}
