//! Media for mode plugins: the ffmpeg/cpal [`player`], the [`audio`] output
//! choice, child [`process`]es without a console window, pictures decoded
//! for a view ([`still`]), media named in text ([`detect`]) and the
//! fullscreen open/close ease ([`anim`]), and [`view`], inline players with
//! fullscreen that any mode can draw. Commander and chat both use these, so
//! a picture or clip plays and flies to fullscreen the same way in each.

pub mod anim;
pub mod audio;
pub mod detect;
pub mod player;
pub mod process;
pub mod still;
pub mod view;

use std::path::PathBuf;
use std::sync::Arc;

/// Asks the plugin to draw again (from a decoder or animation thread).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

static HOME: std::sync::RwLock<Option<fn() -> PathBuf>> = std::sync::RwLock::new(None);

/// Where [`resolve_home`] looks instead (a mode with its own idea of the
/// home, e.g. a test home, passes its function once at start).
pub fn set_home(f: fn() -> PathBuf) {
    *HOME.write().unwrap_or_else(|e| e.into_inner()) = Some(f);
}

/// Mouser's home: the [`set_home`] function, else `MOUSER_HOME`, else `~/.mouser`.
pub fn resolve_home() -> PathBuf {
    if let Some(f) = *HOME.read().unwrap_or_else(|e| e.into_inner()) {
        return f();
    }
    if let Some(h) = std::env::var_os("MOUSER_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    // Tests never touch the real Mouser home.
    if cfg!(test) {
        return std::env::temp_dir().join("mouser-media-tests");
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

    #[test]
    fn a_mode_can_say_where_its_home_is() {
        fn home() -> std::path::PathBuf {
            std::env::temp_dir().join("mouser-media-set-home")
        }
        super::set_home(home);
        assert_eq!(super::resolve_home(), home());
    }
}
