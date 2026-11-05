//! Key mappings, as Mouser reads them from `modes.toml` (a copy of
//! `mouser_core::modes`: the plugin shows the same labels as the host maps).
//! Keys are plain while the mode has the keyboard (the focus toggle).

use std::collections::BTreeMap;
use std::path::PathBuf;

/// A mode's default binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// Action id the mode receives (also the `modes.toml` key).
    pub id: &'static str,
    /// A plain key while the mode has focus: a character, or `up`, `down`,
    /// `left`, `right`, `enter`, `pageup`, `pagedown`, `home`, `end`,
    /// `space`, `backspace`, `tab`; `alt+<char>` and `ctrl+alt+<letter>`
    /// work too.
    pub key: &'static str,
    pub describe: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    /// (id, key, description) in the mode's order.
    pub keys: Vec<(String, String, String)>,
}

/// `modes.toml` in the Mouser home.
pub fn modes_path() -> PathBuf {
    crate::storage::resolve_home().join("modes.toml")
}

#[derive(Debug, Default, serde::Deserialize)]
struct Section {
    /// Before the focus model keys were `alt+<key>`; the setting is ignored.
    #[allow(dead_code)]
    modifier: Option<String>,
    #[serde(default)]
    keys: BTreeMap<String, String>,
}

impl Keymap {
    pub fn new(defaults: &[Binding]) -> Self {
        Self { keys: defaults.iter().map(|b| (b.id.into(), b.key.into(), b.describe.into())).collect() }
    }

    /// Apply `[<mode>]` from `modes.toml` text.
    pub fn apply(&mut self, mode: &str, text: &str) -> Result<(), String> {
        let all: BTreeMap<String, toml::Value> = toml::from_str(text).map_err(|e| e.to_string())?;
        let Some(v) = all.get(mode) else { return Ok(()) };
        let s: Section = v.clone().try_into().map_err(|e: toml::de::Error| e.to_string())?;
        for (id, key) in s.keys {
            if key_bytes(&key).is_none() {
                return Err(format!("unknown key {key:?} for {id}"));
            }
            match self.keys.iter_mut().find(|k| k.0 == id) {
                Some(k) => k.1 = if key.chars().count() == 1 { key } else { key.to_lowercase() },
                None => return Err(format!("{mode} has no action {id:?}")),
            }
        }
        Ok(())
    }

    /// Defaults plus `modes.toml`; a problem is reported and the defaults kept.
    pub fn load(mode: &str, defaults: &[Binding]) -> (Self, Option<String>) {
        let mut k = Self::new(defaults);
        let Ok(t) = std::fs::read_to_string(modes_path()) else { return (k, None) };
        match k.apply(mode, &t) {
            Ok(()) => (k, None),
            Err(e) => (Self::new(defaults), Some(format!("modes.toml [{mode}]: {e}; using the default keys"))),
        }
    }

    /// The action bound to these key bytes (while the mode has focus).
    pub fn lookup(&self, bytes: &[u8]) -> Option<&str> {
        self.keys.iter().find(|(_, k, _)| key_bytes(k).is_some_and(|kb| kb.iter().any(|b| b.as_slice() == bytes))).map(|(id, _, _)| id.as_str())
    }

    /// The key as typed: `s`, `enter`, `ctrl+alt+k`.
    pub fn label(&self, id: &str) -> String {
        self.keys.iter().find(|k| k.0 == id).map_or("?", |k| k.1.as_str()).to_string()
    }
}

/// The one key a mode takes from the shell: it gives the current mode the
/// keyboard and back. Ctrl+Alt+K is bound by none of bash (readline), zsh
/// (zle) or fish in their default emacs keymaps, and no desktop reserves it
/// (GNOME, KDE, Windows and macOS keep Ctrl+Alt+T/L/D/Delete/arrows/F-keys).
/// On Windows Ctrl+Alt is also AltGr: on a layout where AltGr+K types a
/// character, remap it (`[focus] key` in `modes.toml`).
pub const DEFAULT_FOCUS_KEY: &str = "ctrl+alt+k";

/// The focus toggle: `[focus] key` in `modes.toml`, else
/// [`DEFAULT_FOCUS_KEY`]. A problem is reported and the default kept.
pub fn load_focus_key() -> (String, Option<String>) {
    let Ok(t) = std::fs::read_to_string(modes_path()) else { return (DEFAULT_FOCUS_KEY.into(), None) };
    match focus_key_from(&t) {
        Ok(k) => (k, None),
        Err(e) => (DEFAULT_FOCUS_KEY.into(), Some(format!("modes.toml [focus]: {e}; using {DEFAULT_FOCUS_KEY}"))),
    }
}

/// `[focus] key` from `modes.toml` text. It must be a modified key (a plain
/// key would never reach the shell).
pub fn focus_key_from(text: &str) -> Result<String, String> {
    let all: BTreeMap<String, toml::Value> = toml::from_str(text).map_err(|e| e.to_string())?;
    let Some(k) = all.get("focus").and_then(|f| f.get("key")) else { return Ok(DEFAULT_FOCUS_KEY.into()) };
    let k = k.as_str().ok_or("key must be a string")?.to_lowercase();
    if !(k.starts_with("alt+") || k.starts_with("ctrl+alt+")) || key_bytes(&k).is_none() {
        return Err(format!("{k:?} is not an alt+<key> or ctrl+alt+<letter> key"));
    }
    Ok(k)
}

/// Run `cmd` in an emacs-keymap line editor (bash/readline, zsh/zle, fish)
/// without losing what is typed: Ctrl+E, a space, Ctrl+U cuts the whole line
/// (plus the space, so the kill is never empty and an old kill is never
/// yanked) into the kill ring, then the command and Enter. Send
/// [`RESTORE_LINE`] once the shell has its prompt back.
pub fn keep_line(cmd: &str) -> Vec<u8> {
    let mut v = b"\x05 \x15".to_vec();
    v.extend(cmd.replace(['\r', '\n'], " ").as_bytes());
    v.push(b'\r');
    v
}

/// Ctrl+Y yanks the line back, Backspace drops the added space.
pub const RESTORE_LINE: &[u8] = b"\x19\x7f";

/// What `key` sends (some keys have more than one encoding: normal and
/// application cursor mode, xterm and rxvt Home/End).
pub fn key_bytes(key: &str) -> Option<Vec<Vec<u8>>> {
    let k = key.to_lowercase();
    if let Some(l) = k.strip_prefix("ctrl+alt+") {
        // ctrl+alt+letter: ESC and the control character.
        return (l.len() == 1 && l.as_bytes()[0].is_ascii_lowercase()).then(|| vec![vec![0x1b, l.as_bytes()[0] & 0x1f]]);
    }
    if k.starts_with("alt+") {
        return Some(key_bytes(&key[4..])?.into_iter().map(|b| [&[0x1b][..], &b].concat()).collect());
    }
    Some(match k.as_str() {
        "up" => vec![b"\x1b[A".to_vec(), b"\x1bOA".to_vec()],
        "down" => vec![b"\x1b[B".to_vec(), b"\x1bOB".to_vec()],
        "right" => vec![b"\x1b[C".to_vec(), b"\x1bOC".to_vec()],
        "left" => vec![b"\x1b[D".to_vec(), b"\x1bOD".to_vec()],
        "enter" => vec![b"\r".to_vec()],
        "tab" => vec![b"\t".to_vec()],
        "pageup" => vec![b"\x1b[5~".to_vec()],
        "pagedown" => vec![b"\x1b[6~".to_vec()],
        "home" => vec![b"\x1b[H".to_vec(), b"\x1bOH".to_vec(), b"\x1b[1~".to_vec()],
        "end" => vec![b"\x1b[F".to_vec(), b"\x1bOF".to_vec(), b"\x1b[4~".to_vec()],
        "space" => vec![b" ".to_vec()],
        "backspace" => vec![b"\x7f".to_vec(), b"\x08".to_vec()],
        // One character keeps its case: `F` is Shift+F, not `f`.
        _ => {
            let mut c = key.chars();
            let ch = c.next()?;
            if c.next().is_some() || ch.is_control() || ch == ' ' {
                return None;
            }
            vec![ch.to_string().into_bytes()]
        }
    })
}
