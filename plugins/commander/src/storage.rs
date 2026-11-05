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

/// `[audio]` from `preferences.toml`: (host, output_device).
pub fn audio_prefs() -> (Option<String>, Option<String>) {
    let Ok(s) = std::fs::read_to_string(resolve_home().join("preferences.toml")) else { return (None, None) };
    let Ok(v) = s.parse::<toml::Table>() else { return (None, None) };
    let a = v.get("audio").and_then(|a| a.as_table());
    let get = |k: &str| a.and_then(|a| a.get(k)).and_then(|x| x.as_str()).map(str::to_string);
    (get("host"), get("output_device"))
}

#[cfg(test)]
mod tests {
    #[test]
    #[serial_test::serial]
    fn audio_prefs_come_from_the_preferences_file() {
        let _g = crate::view::tests::ENV.lock().unwrap_or_else(|e| e.into_inner());
        let t = tempfile::tempdir().unwrap();
        // SAFETY: serialised with the other tests that set MOUSER_HOME.
        unsafe { std::env::set_var("MOUSER_HOME", t.path()) };
        assert_eq!(super::audio_prefs(), (None, None));
        std::fs::write(t.path().join("preferences.toml"), "theme = \"Nord\"\n[audio]\nhost = \"jack\"\noutput_device = \"pipewire\"\n").unwrap();
        assert_eq!(super::audio_prefs(), (Some("jack".into()), Some("pipewire".into())));
        unsafe { std::env::remove_var("MOUSER_HOME") };
    }
}
