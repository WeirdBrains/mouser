//! Input handling and validation for the in-app theme switcher and editor.
//! Front ends translate key presses into [`EditKey`] and render the state.

use crate::storage::ThemeMode;
use crate::themes::{ThemeDef, parse_hex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKey {
    Char(char),
    Backspace,
    Next,
    Prev,
    Up,
    Down,
    Left,
    Right,
    Enter,
    Save,
    Cancel,
}

/// Map terminal-style key bytes (as produced by the renderer) to an [`EditKey`].
pub fn parse_key(bytes: &[u8]) -> Option<EditKey> {
    Some(match bytes {
        b"\x1b" => EditKey::Cancel,
        b"\r" | b"\n" => EditKey::Enter,
        b"\t" => EditKey::Next,
        b"\x1b[Z" => EditKey::Prev,
        b"\x1b[A" => EditKey::Up,
        b"\x1b[B" => EditKey::Down,
        b"\x1b[C" => EditKey::Right,
        b"\x1b[D" => EditKey::Left,
        b"\x7f" | b"\x08" => EditKey::Backspace,
        b"\x13" => EditKey::Save,
        _ => {
            let s = std::str::from_utf8(bytes).ok()?;
            let mut it = s.chars();
            let c = it.next()?;
            if it.next().is_some() || c.is_control() {
                return None;
            }
            EditKey::Char(c)
        }
    })
}

/// What the front end should do after a key.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Nothing visible changed beyond the form itself.
    Continue,
    /// Show the current choice live, without saving.
    Preview,
    /// Persist the current choice and close.
    Save,
    /// Discard and close (revert any preview).
    Close,
    /// Switch from the picker to the editor.
    OpenEditor,
}

pub fn mode_name(m: &ThemeMode) -> &'static str {
    match m {
        ThemeMode::Dark => "dark",
        ThemeMode::Light => "light",
        ThemeMode::System => "system",
    }
}

pub fn parse_mode(s: &str) -> Option<ThemeMode> {
    match s.trim().to_ascii_lowercase().as_str() {
        "dark" => Some(ThemeMode::Dark),
        "light" => Some(ThemeMode::Light),
        "system" | "auto" => Some(ThemeMode::System),
        _ => None,
    }
}

const MODES: [ThemeMode; 3] = [ThemeMode::Dark, ThemeMode::Light, ThemeMode::System];

/// Theme and mode switcher.
#[derive(Debug, Clone)]
pub struct ThemePicker {
    pub themes: Vec<String>,
    pub selected: usize,
    pub mode: ThemeMode,
}

impl ThemePicker {
    pub fn new(themes: Vec<String>, current: &str, mode: ThemeMode) -> Self {
        let selected = themes.iter().position(|t| t == current).unwrap_or(0);
        Self { themes, selected, mode }
    }

    pub fn theme(&self) -> &str {
        self.themes.get(self.selected).map(String::as_str).unwrap_or("Red Star")
    }

    pub fn select(&mut self, i: usize) -> Outcome {
        if i < self.themes.len() && i != self.selected {
            self.selected = i;
            return Outcome::Preview;
        }
        Outcome::Continue
    }

    pub fn set_mode(&mut self, m: ThemeMode) -> Outcome {
        if m != self.mode {
            self.mode = m;
            return Outcome::Preview;
        }
        Outcome::Continue
    }

    pub fn handle(&mut self, key: EditKey) -> Outcome {
        let n = self.themes.len().max(1);
        match key {
            EditKey::Up | EditKey::Prev => self.select((self.selected + n - 1) % n),
            EditKey::Down | EditKey::Next => self.select((self.selected + 1) % n),
            EditKey::Left | EditKey::Right => {
                let i = MODES.iter().position(|m| *m == self.mode).unwrap_or(0);
                let j = if key == EditKey::Right { (i + 1) % 3 } else { (i + 2) % 3 };
                self.set_mode(MODES[j].clone())
            }
            EditKey::Char('d') => self.set_mode(ThemeMode::Dark),
            EditKey::Char('l') => self.set_mode(ThemeMode::Light),
            EditKey::Char('s') => self.set_mode(ThemeMode::System),
            EditKey::Char('e') => Outcome::OpenEditor,
            EditKey::Enter | EditKey::Save => Outcome::Save,
            EditKey::Cancel => Outcome::Close,
            _ => Outcome::Continue,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// Hex colour; `optional` fields may be left empty.
    Color {
        optional: bool,
    },
    Text,
    /// Font size in points.
    Size,
    /// TCP port (empty allowed).
    Port,
    /// SSH Host alias: one word, no wildcards.
    Alias,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub label: String,
    pub value: String,
    pub kind: FieldKind,
}

impl Field {
    pub fn new(label: &str, value: impl Into<String>, kind: FieldKind) -> Self {
        Self { label: label.to_string(), value: value.into(), kind }
    }

    /// Whether the current text is acceptable.
    pub fn is_valid(&self) -> bool {
        match self.kind {
            FieldKind::Color { optional } => (optional && self.value.trim().is_empty()) || parse_hex(&self.value).is_some(),
            FieldKind::Text => true,
            FieldKind::Size => parse_size(&self.value).is_some(),
            FieldKind::Port => self.value.is_empty() || self.value.parse::<u16>().is_ok_and(|p| p > 0),
            FieldKind::Alias => crate::ssh_config::valid_alias(&self.value),
        }
    }

    /// Colour for a swatch, if this is a valid colour field.
    pub fn swatch(&self) -> Option<u32> {
        match self.kind {
            FieldKind::Color { .. } => parse_hex(&self.value),
            _ => None,
        }
    }

    pub fn accepts(&self, c: char) -> bool {
        match self.kind {
            FieldKind::Color { .. } => (c == '#' || c.is_ascii_hexdigit()) && self.value.len() < 7,
            FieldKind::Size => (c.is_ascii_digit() || c == '.') && self.value.len() < 5,
            FieldKind::Text => !c.is_control() && self.value.chars().count() < 256,
            FieldKind::Port => c.is_ascii_digit() && self.value.len() < 5,
            FieldKind::Alias => !c.is_whitespace() && !"*?!\"#=".contains(c) && self.value.chars().count() < 64,
        }
    }
}

pub const MIN_FONT_SIZE: f32 = 6.0;
pub const MAX_FONT_SIZE: f32 = 48.0;

pub fn parse_size(s: &str) -> Option<f32> {
    let v: f32 = s.trim().parse().ok()?;
    (MIN_FONT_SIZE..=MAX_FONT_SIZE).contains(&v).then_some(v)
}

/// Indices into [`ThemeEditor::fields`].
pub const FG: usize = 0;
pub const BG: usize = 1;
pub const CURSOR: usize = 2;
pub const ACCENT: usize = 3;
pub const PALETTE: usize = 4; // 16 entries
pub const FONT_FAMILY: usize = 20;
pub const FONT_SIZE: usize = 21;

/// A validated editor result.
#[derive(Debug, Clone, PartialEq)]
pub struct EditorResult {
    pub theme: ThemeDef,
    pub font_family: Option<String>,
    pub font_size: f32,
}

/// Form for the active theme variant's colours plus font family and size.
#[derive(Debug, Clone)]
pub struct ThemeEditor {
    pub theme: String,
    pub variant: String,
    pub fields: Vec<Field>,
    pub focus: usize,
    pub error: Option<String>,
}

impl ThemeEditor {
    pub fn new(def: &ThemeDef, font_family: Option<&str>, font_size: f32) -> Self {
        let mut fields = vec![
            Field::new("Foreground", def.foreground.clone(), FieldKind::Color { optional: false }),
            Field::new("Background", def.background.clone(), FieldKind::Color { optional: false }),
            Field::new("Cursor", def.cursor.clone().unwrap_or_default(), FieldKind::Color { optional: true }),
            Field::new("Accent", def.accent.clone().unwrap_or_default(), FieldKind::Color { optional: true }),
        ];
        for (i, c) in def.full_palette().into_iter().enumerate() {
            fields.push(Field::new(&format!("{i}"), c, FieldKind::Color { optional: false }));
        }
        fields.push(Field::new("Font family", font_family.unwrap_or(""), FieldKind::Text));
        fields.push(Field::new("Font size", format_size(font_size), FieldKind::Size));
        Self { theme: def.name.clone(), variant: def.variant.clone(), fields, focus: 0, error: None }
    }

    pub fn focus_field(&mut self, i: usize) {
        if i < self.fields.len() {
            self.focus = i;
        }
    }

    pub fn handle(&mut self, key: EditKey) -> Outcome {
        let n = self.fields.len();
        match key {
            EditKey::Next | EditKey::Down | EditKey::Enter => {
                self.focus = (self.focus + 1) % n;
                Outcome::Continue
            }
            EditKey::Prev | EditKey::Up => {
                self.focus = (self.focus + n - 1) % n;
                Outcome::Continue
            }
            EditKey::Left | EditKey::Right => Outcome::Continue,
            EditKey::Cancel => Outcome::Close,
            EditKey::Save => match self.validate() {
                Ok(_) => {
                    self.error = None;
                    Outcome::Save
                }
                Err(e) => {
                    self.error = Some(e);
                    Outcome::Continue
                }
            },
            EditKey::Backspace => {
                self.fields[self.focus].value.pop();
                self.after_edit()
            }
            EditKey::Char(c) => {
                if !self.fields[self.focus].accepts(c) {
                    return Outcome::Continue;
                }
                self.fields[self.focus].value.push(c);
                self.after_edit()
            }
        }
    }

    fn after_edit(&mut self) -> Outcome {
        match self.validate() {
            Ok(_) => {
                self.error = None;
                Outcome::Preview
            }
            Err(e) => {
                self.error = Some(e);
                Outcome::Continue
            }
        }
    }

    /// Check every field; on success return the theme and font to apply.
    pub fn validate(&self) -> Result<EditorResult, String> {
        for f in &self.fields {
            if !f.is_valid() {
                return Err(match f.kind {
                    FieldKind::Size => format!("Font size must be {MIN_FONT_SIZE}–{MAX_FONT_SIZE}"),
                    _ if f.label.len() <= 2 => format!("Palette {}: use #rrggbb", f.label),
                    _ => format!("{}: use #rrggbb", f.label),
                });
            }
        }
        let norm = |s: &str| parse_hex(s).map(|v| format!("#{v:06X}"));
        let v = |i: usize| self.fields[i].value.trim().to_string();
        let theme = ThemeDef {
            name: self.theme.clone(),
            variant: self.variant.clone(),
            foreground: norm(&v(FG)).unwrap(),
            background: norm(&v(BG)).unwrap(),
            cursor: norm(&v(CURSOR)),
            accent: norm(&v(ACCENT)),
            palette: (PALETTE..PALETTE + 16).map(|i| norm(&v(i)).unwrap()).collect(),
        };
        let family = v(FONT_FAMILY);
        Ok(EditorResult { theme, font_family: (!family.is_empty()).then_some(family), font_size: parse_size(&v(FONT_SIZE)).unwrap() })
    }
}

pub fn format_size(v: f32) -> String {
    if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v:.1}") }
}
