//! Shell integration scripts (`mouser shell-integration SHELL`): OSC 7 and the
//! Fuzzball `s` subsystem for silent directory changes (docs/protocol/shell.md).

pub const ZSH: &str = include_str!("../shell/mouser.zsh");
pub const BASH: &str = include_str!("../shell/mouser.bash");
pub const FISH: &str = include_str!("../shell/mouser.fish");

/// The script for `shell` (`zsh`, `bash` or `fish`).
pub fn script(shell: &str) -> Option<&'static str> {
    match shell {
        "zsh" => Some(ZSH),
        "bash" => Some(BASH),
        "fish" => Some(FISH),
        _ => None,
    }
}
