//! Mode key mappings (MODES.md, docs/modes.md). Modes are plugins (see
//! `mouser_render::modes`); each declares its bindings, every binding is
//! `<modifier>+<key>` and can be remapped in `modes.toml`:
//!
//! ```toml
//! [commander]
//! modifier = "alt"
//! [commander.keys]
//! switch_view = "v"
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

/// A mode's default binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// Action id the mode receives (also the `modes.toml` key).
    pub id: &'static str,
    /// A character, or `up`, `down`, `left`, `right`, `enter`, `pageup`,
    /// `pagedown`, `home`, `end`, `space`, `backspace`.
    pub key: &'static str,
    pub describe: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    pub modifier: String,
    /// (id, key, description) in the mode's order.
    pub keys: Vec<(String, String, String)>,
}

/// `modes.toml` in the Mouser home.
pub fn modes_path() -> PathBuf {
    crate::storage::resolve_home().join("modes.toml")
}

#[derive(Debug, Default, serde::Deserialize)]
struct Section {
    modifier: Option<String>,
    #[serde(default)]
    keys: BTreeMap<String, String>,
}

impl Keymap {
    pub fn new(defaults: &[Binding]) -> Self {
        Self { modifier: "alt".into(), keys: defaults.iter().map(|b| (b.id.into(), b.key.into(), b.describe.into())).collect() }
    }

    /// Apply `[<mode>]` from `modes.toml` text. Only the `alt` modifier is
    /// supported for now.
    pub fn apply(&mut self, mode: &str, text: &str) -> Result<(), String> {
        let all: BTreeMap<String, toml::Value> = toml::from_str(text).map_err(|e| e.to_string())?;
        let Some(v) = all.get(mode) else { return Ok(()) };
        let s: Section = v.clone().try_into().map_err(|e: toml::de::Error| e.to_string())?;
        if let Some(m) = s.modifier {
            if m != "alt" {
                return Err(format!("modifier {m:?} is not supported yet (only \"alt\")"));
            }
            self.modifier = m;
        }
        for (id, key) in s.keys {
            if key_bytes(&key).is_none() {
                return Err(format!("unknown key {key:?} for {id}"));
            }
            match self.keys.iter_mut().find(|k| k.0 == id) {
                Some(k) => k.1 = key.to_lowercase(),
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

    /// The action bound to these key bytes. Anything else (other alt keys
    /// included) is not the mode's and goes to the shell.
    pub fn lookup(&self, bytes: &[u8]) -> Option<&str> {
        self.keys.iter().find(|(_, k, _)| key_bytes(k).is_some_and(|kb| kb.iter().any(|b| b.as_slice() == bytes))).map(|(id, _, _)| id.as_str())
    }

    /// `alt+t`.
    pub fn label(&self, id: &str) -> String {
        format!("{}+{}", self.modifier, self.keys.iter().find(|k| k.0 == id).map_or("?", |k| k.1.as_str()))
    }
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

/// What alt+`key` sends (some keys have more than one encoding).
pub fn key_bytes(key: &str) -> Option<Vec<Vec<u8>>> {
    let k = key.to_lowercase();
    Some(match k.as_str() {
        "up" => vec![b"\x1b[1;3A".to_vec(), b"\x1b\x1b[A".to_vec()],
        "down" => vec![b"\x1b[1;3B".to_vec(), b"\x1b\x1b[B".to_vec()],
        "right" => vec![b"\x1b[1;3C".to_vec(), b"\x1b\x1b[C".to_vec()],
        "left" => vec![b"\x1b[1;3D".to_vec(), b"\x1b\x1b[D".to_vec()],
        "enter" => vec![b"\x1b\r".to_vec()],
        "pageup" => vec![b"\x1b[5;3~".to_vec(), b"\x1b\x1b[5~".to_vec()],
        "pagedown" => vec![b"\x1b[6;3~".to_vec(), b"\x1b\x1b[6~".to_vec()],
        "home" => vec![b"\x1b[1;3H".to_vec(), b"\x1b\x1b[H".to_vec(), b"\x1b\x1b[1~".to_vec()],
        "end" => vec![b"\x1b[1;3F".to_vec(), b"\x1b\x1b[F".to_vec(), b"\x1b\x1b[4~".to_vec()],
        "space" => vec![b"\x1b ".to_vec()],
        "backspace" => vec![b"\x1b\x7f".to_vec(), b"\x1b\x08".to_vec()],
        _ => {
            let mut c = k.chars();
            let ch = c.next()?;
            if c.next().is_some() || ch.is_control() || ch == ':' {
                return None;
            }
            let mut b = vec![0x1b];
            b.extend(ch.to_string().as_bytes());
            vec![b]
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
        Binding { id: "switch_view", key: "t", describe: "switch view" },
        Binding { id: "up", key: "up", describe: "up" },
        Binding { id: "open", key: "enter", describe: "open" },
    ];

    #[test]
    fn takes_only_bound_keys_and_remaps() {
        let k = Keymap::new(B);
        assert_eq!(k.lookup(b"\x1bt"), Some("switch_view"));
        assert_eq!(k.lookup(b"\x1b[1;3A"), Some("up"));
        assert_eq!(k.lookup(b"\x1b\r"), Some("open"));
        for other in [&b"\x1bb"[..], b"\x1bf", b"\x1b.", b"\x1bd", b"\x1b/", b"t", b"\x1b"] {
            assert_eq!(k.lookup(other), None, "{other:?} goes to the shell");
        }
        let mut r = Keymap::new(B);
        r.apply("commander", "[commander]\nmodifier = \"alt\"\n[commander.keys]\nswitch_view = \"v\"\n[other.keys]\nx = \"y\"\n").unwrap();
        assert_eq!(r.lookup(b"\x1bv"), Some("switch_view"));
        assert_eq!(r.lookup(b"\x1bt"), None);
        assert_eq!(r.label("switch_view"), "alt+v");
        assert!(Keymap::new(B).apply("commander", "[commander]\nmodifier = \"ctrl\"\n").is_err());
        assert!(Keymap::new(B).apply("commander", "[commander.keys]\nup = \"f1\"\n").is_err());
        assert!(Keymap::new(B).apply("commander", "[commander.keys]\nnope = \"q\"\n").is_err());
        assert!(Keymap::new(B).apply("commander", "[commander.keys]\nup = \":\"\n").is_err(), "':' is esc-: itself");
    }
}
