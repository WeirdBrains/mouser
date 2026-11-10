//! Media for mode plugins: the ffmpeg/cpal [`player`], the [`audio`] output
//! choice, child [`process`]es without a console window, pictures decoded
//! for a view ([`still`]), media named in text ([`detect`]) and the
//! fullscreen open/close ease ([`anim`]). Chat uses these and commander will,
//! so a picture or clip plays and flies to fullscreen the same way in each
//! (commander still carries its own copy until it moves onto this crate).

pub mod anim;
pub mod audio;
pub mod detect;
pub mod player;
pub mod process;
pub mod still;

use std::path::PathBuf;
use std::sync::Arc;

/// Asks the plugin to draw again (from a decoder or animation thread).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Mouser's home (`MOUSER_HOME`, else `~/.mouser`).
pub fn resolve_home() -> PathBuf {
    if let Some(h) = std::env::var_os("MOUSER_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".mouser")
}

/// `[audio]` from `preferences.toml`: (host, output_device).
pub fn audio_prefs() -> (Option<String>, Option<String>) {
    audio_prefs_in(&resolve_home())
}

fn audio_prefs_in(home: &std::path::Path) -> (Option<String>, Option<String>) {
    let Ok(s) = std::fs::read_to_string(home.join("preferences.toml")) else { return (None, None) };
    let Ok(v) = s.parse::<toml::Table>() else { return (None, None) };
    let a = v.get("audio").and_then(|a| a.as_table());
    let get = |k: &str| a.and_then(|a| a.get(k)).and_then(|x| x.as_str()).map(str::to_string);
    (get("host"), get("output_device"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn audio_prefs_come_from_the_preferences_file() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(super::audio_prefs_in(t.path()), (None, None));
        std::fs::write(t.path().join("preferences.toml"), "theme = \"Nord\"\n[audio]\nhost = \"jack\"\noutput_device = \"pipewire\"\n").unwrap();
        assert_eq!(super::audio_prefs_in(t.path()), (Some("jack".into()), Some("pipewire".into())));
    }
}
