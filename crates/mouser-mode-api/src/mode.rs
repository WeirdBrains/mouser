//! What a mode asks of the host, and where it may draw.

use crate::ui::{Rect, Viewport};
use std::path::PathBuf;

/// Where a mode may draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Context {
    pub viewport: Viewport,
    /// The terminal area (left of the sidebar, below the tab strip); the view
    /// takes its top part and the terminal grid moves down.
    pub area: Rect,
}

/// What the mode asks Mouser to do. The host checks each one against the
/// plugin's granted permissions (`Type` and `Run` need `shell.type`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Effect {
    None,
    /// Type this into the shell.
    Type(String),
    /// Run a command line in the shell; what the user has typed is kept
    /// (cut to the kill ring and yanked back when the prompt returns).
    Run(String),
    /// Change the shell's directory: silently through the shell integration
    /// (Fuzzball `s c`, fuzzball docs/shell.md), else by typing `cd` and then
    /// erasing those lines so one prompt remains.
    Cd(PathBuf),
    /// Hand a file to the desktop's opener (players etc.).
    Open(PathBuf),
    /// Open a link (a Markdown link clicked in chat): web, mail and file
    /// links only, never through a shell.
    OpenLink(String),
    /// Show the key mappings window.
    ShowKeys,
    /// Status-bar message.
    Say(String),
    /// Not handled now: the key goes to the shell.
    Pass,
}

impl Effect {
    /// The permission the host requires before carrying this out.
    pub fn permission(&self) -> Option<&'static str> {
        match self {
            Effect::Type(_) | Effect::Run(_) | Effect::Cd(_) => Some("shell.type"),
            Effect::Open(_) => Some("files.read"),
            _ => None,
        }
    }
}
