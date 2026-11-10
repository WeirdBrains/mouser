//! Mode key mappings (MODES.md, docs/modes.md). Modes are plugins (see
//! `mouser_render::modes`); each declares its bindings and every binding can
//! be remapped in `modes.toml`.
//!
//! Focus model: a mode never takes a key from the shell on its own. One key,
//! the focus toggle ([`DEFAULT_FOCUS_KEY`], free in bash, zsh and fish),
//! gives the mode the keyboard; while it has focus its bindings are plain
//! keys (`s`, `/`, arrows, Enter ...) and nothing typed reaches the shell.
//! Esc (or the toggle again) gives the keyboard back.
//!
//! ```toml
//! [focus]
//! key = "ctrl+alt+k"
//! [commander.keys]
//! switch_view = "t"
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

/// A mode's default binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// Action id the mode receives (also the `modes.toml` key).
    pub id: &'static str,
    /// A plain key while the mode has focus: a character, or `up`, `down`,
    /// `left`, `right`, `enter`, `pageup`, `pagedown`, `home`, `end`,
    /// `space`, `backspace`, `tab`, `f1`; `alt+<char>` and `ctrl+alt+<letter>`
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
        "f1" => vec![b"\x1bOP".to_vec(), b"\x1b[11~".to_vec()],
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

/// The mode a window was in when it closed, kept in `$MOUSER_HOME/state/last-mode`
/// (state, not config): a mode name, or `none` for no mode. The last window to
/// close wins.
pub fn last_mode_path() -> PathBuf {
    crate::storage::resolve_home().join("state").join("last-mode")
}

/// `None`: nothing saved (first run): use the default mode. `Some(None)`: no mode.
pub fn load_last_mode() -> Option<Option<String>> {
    let s = std::fs::read_to_string(last_mode_path()).ok()?;
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    Some((s != "none").then(|| s.to_string()))
}

pub fn save_last_mode(mode: Option<&str>) -> std::io::Result<()> {
    let p = last_mode_path();
    std::fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, format!("{}\n", mode.unwrap_or("none")))?;
    std::fs::rename(tmp, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: &[Binding] = &[
        Binding { id: "switch_view", key: "v", describe: "switch view" },
        Binding { id: "up", key: "up", describe: "up" },
        Binding { id: "open", key: "enter", describe: "open" },
    ];

    #[test]
    fn plain_keys_and_remaps() {
        let k = Keymap::new(B);
        assert_eq!(k.lookup(b"v"), Some("switch_view"));
        assert_eq!(k.lookup(b"\x1b[A"), Some("up"));
        assert_eq!(k.lookup(b"\x1bOA"), Some("up"), "application cursor mode");
        assert_eq!(k.lookup(b"\r"), Some("open"));
        for other in [&b"\x1bv"[..], b"x", b"\x1b"] {
            assert_eq!(k.lookup(other), None, "{other:?}");
        }
        assert_eq!(k.label("switch_view"), "v");
        let mut r = Keymap::new(B);
        r.apply("commander", "[commander]\nmodifier = \"alt\"\n[commander.keys]\nswitch_view = \"t\"\n[other.keys]\nx = \"y\"\n").unwrap();
        assert_eq!(r.lookup(b"t"), Some("switch_view"), "an old modifier setting is ignored");
        assert_eq!(r.lookup(b"v"), None);
        r.apply("commander", "[commander.keys]\nup = \"alt+k\"\nopen = \"ctrl+alt+o\"\n").unwrap();
        assert_eq!(r.lookup(b"\x1bk"), Some("up"));
        assert_eq!(r.lookup(b"\x1b\x0f"), Some("open"));
        assert_eq!(r.label("open"), "ctrl+alt+o");
        assert!(Keymap::new(B).apply("commander", "[commander.keys]\nup = \"f13\"\n").is_err());
        assert!(Keymap::new(B).apply("commander", "[commander.keys]\nnope = \"q\"\n").is_err());
        assert!(key_bytes("ctrl+alt+1").is_none() && key_bytes("ctrl+alt+ab").is_none());
    }

    #[test]
    fn focus_key_is_configurable_but_never_plain() {
        assert_eq!(focus_key_from("").unwrap(), DEFAULT_FOCUS_KEY);
        assert_eq!(focus_key_from("[focus]\nkey = \"ctrl+alt+n\"\n").unwrap(), "ctrl+alt+n");
        assert_eq!(focus_key_from("[focus]\nkey = \"alt+pagedown\"\n").unwrap(), "alt+pagedown");
        assert!(focus_key_from("[focus]\nkey = \"k\"\n").is_err());
        assert!(focus_key_from("[focus]\nkey = \"ctrl+alt+1\"\n").is_err());
    }

    /// Bytes for the shells' default emacs-mode bindings that start with
    /// ESC (alt+key and ctrl+alt+key), from `bind -p` (bash 5.2), `bindkey`
    /// (zsh 5.9) and `bind` (fish 4) with empty rc files.
    fn shell_defaults() -> Vec<Vec<u8>> {
        let mut v: Vec<Vec<u8>> = Vec::new();
        let alt = |s: &str, v: &mut Vec<Vec<u8>>| v.extend(s.chars().map(|c| format!("\x1b{c}").into_bytes()));
        let ctrl_alt = |s: &str, v: &mut Vec<Vec<u8>>| v.extend(s.bytes().map(|c| vec![0x1b, c & 0x1f]));
        // bash / readline
        alt(" !#&*-./<=>?@\\^_{~$0123456789bcdfglnprtuxy", &mut v);
        ctrl_alt("ghlirbefdty]", &mut v);
        v.push(b"\x1b\x1f".to_vec()); // M-C-_
        v.push(b"\x1b\x7f".to_vec()); // M-DEL
        // zsh / zle
        alt(" !\"$'-.<>?_|0123456789abcdfghlnpqstuwxyzABCDFGHLNPQSTUWXYZ", &mut v);
        ctrl_alt("dghijlm_", &mut v);
        // fish
        alt("#./<>bcdefhlopstuvwy", &mut v);
        ctrl_alt("h", &mut v);
        v.push(b"\x1b\r".to_vec());
        for k in ["up", "down", "left", "right", "backspace"] {
            v.extend(key_bytes(&format!("alt+{k}")).unwrap());
        }
        v.extend([b"\x1b[1;3A".to_vec(), b"\x1b[1;3B".to_vec(), b"\x1b[1;3C".to_vec(), b"\x1b[1;3D".to_vec(), b"\x1b[3;3~".to_vec()]);
        v
    }

    /// Ctrl+Alt keys desktops keep for themselves (GNOME, KDE, Windows).
    const DESKTOP: &[&str] = &["ctrl+alt+t", "ctrl+alt+l", "ctrl+alt+d"];

    #[test]
    fn the_focus_key_is_no_shell_or_desktop_default() {
        let shells = shell_defaults();
        let focus = key_bytes(DEFAULT_FOCUS_KEY).unwrap();
        for b in &focus {
            assert!(!shells.contains(b), "{DEFAULT_FOCUS_KEY} is a bash/zsh/fish default");
        }
        assert!(!DESKTOP.contains(&DEFAULT_FOCUS_KEY));
        // The table itself catches the keys that were collisions before.
        for k in ["alt+t", "alt+s", "alt+f", "ctrl+alt+f", "alt+up", "alt+enter"] {
            assert!(key_bytes(k).unwrap().iter().any(|b| shells.contains(b)), "{k}");
        }
    }
}

#[cfg(test)]
mod case_tests {
    #[test]
    fn one_character_keys_keep_their_case() {
        assert_eq!(super::key_bytes("F").unwrap(), vec![b"F".to_vec()]);
        assert_eq!(super::key_bytes("f").unwrap(), vec![b"f".to_vec()]);
        assert_eq!(super::key_bytes("alt+F").unwrap(), vec![b"\x1bF".to_vec()]);
        assert_eq!(super::key_bytes("Enter").unwrap(), vec![b"\r".to_vec()]);
        let mut k = super::Keymap::new(&[super::Binding { id: "a", key: "x", describe: "" }]);
        k.apply("m", "[m.keys]\na = \"X\"\n").unwrap();
        assert_eq!(k.lookup(b"X"), Some("a"));
        assert_eq!(k.lookup(b"x"), None);
    }
}
