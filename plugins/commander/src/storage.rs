//! Mouser's home (`MOUSER_HOME`, else `~/.mouser`): the plugin keeps its
//! caches (thumbnails, remote files) there, like the built-in mode did.

use std::path::PathBuf;

pub fn resolve_home() -> PathBuf {
    if let Ok(h) = std::env::var("MOUSER_HOME") {
        return PathBuf::from(h);
    }
    // Tests never touch the real Mouser home.
    if cfg!(test) {
        return std::env::temp_dir().join("mouser-commander-tests");
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".mouser")
}
