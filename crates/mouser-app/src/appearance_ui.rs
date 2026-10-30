//! Thin adapter between the core theme switcher/editor and the renderer.

use mouser_core::appearance::{self, EditKey, Outcome, ThemeEditor, ThemePicker};
use mouser_core::storage::{Preferences, ThemeMode, load_preferences, update_preferences};
use mouser_core::themes::{ThemeColors, active_variant, list_themes, load_theme, resolve_active_theme, save_theme};
use mouser_render::{Colors, ui};
use std::time::{Duration, Instant};

pub enum Panel {
    Picker(ThemePicker),
    Editor(ThemeEditor),
}

/// Live appearance state: fonts, colours (with preview) and the open panel.
pub struct Appearance {
    pub panel: Option<Panel>,
    pub font_size: f32,
    pub font_family: Option<String>,
    /// Font size from config.toml (Ctrl+0 resets to it).
    pub default_font_size: f32,
    preview_colors: Option<ThemeColors>,
    preview_font: Option<(f32, Option<String>)>,
    cache: Option<(Instant, Colors)>,
    pub message: Option<(String, Instant)>,
}

fn to_colors(c: ThemeColors) -> Colors {
    Colors { bg: c.background, fg: c.foreground, cursor: c.cursor, accent: c.accent }
}

fn variant_for(mode: &ThemeMode) -> &'static str {
    active_variant(&Preferences { mode: Some(mode.clone()), ..Default::default() })
}

impl Appearance {
    pub fn new(font_size: f32, font_family: Option<String>, default_font_size: f32) -> Self {
        Self { panel: None, font_size, font_family, default_font_size, preview_colors: None, preview_font: None, cache: None, message: None }
    }

    pub fn colors(&mut self) -> Colors {
        if let Some(c) = self.preview_colors {
            return to_colors(c);
        }
        match self.cache {
            Some((at, c)) if at.elapsed() < Duration::from_secs(1) => c,
            _ => {
                let c = to_colors(resolve_active_theme(&load_preferences()).colors());
                self.cache = Some((Instant::now(), c));
                c
            }
        }
    }

    pub fn font(&self) -> (f32, Option<String>) {
        self.preview_font.clone().unwrap_or((self.font_size, self.font_family.clone()))
    }

    pub fn say(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), Instant::now()));
    }

    pub fn status(&self) -> Option<String> {
        self.message.as_ref().filter(|(_, at)| at.elapsed() < Duration::from_secs(4)).map(|(m, _)| m.clone())
    }

    fn invalidate(&mut self) {
        self.cache = None;
        self.preview_colors = None;
        self.preview_font = None;
    }

    pub fn open_picker(&mut self) {
        let prefs = load_preferences();
        let current = prefs.theme.clone().unwrap_or_else(|| "Red Star".into());
        self.invalidate();
        self.panel = Some(Panel::Picker(ThemePicker::new(list_themes(), &current, prefs.mode.unwrap_or_default())));
    }

    pub fn open_editor(&mut self) {
        let prefs = load_preferences();
        let def = match &self.panel {
            Some(Panel::Picker(p)) => load_theme(p.theme(), variant_for(&p.mode)),
            _ => resolve_active_theme(&prefs),
        };
        self.panel = Some(Panel::Editor(ThemeEditor::new(&def, self.font_family.as_deref(), self.font_size)));
    }

    pub fn key(&mut self, bytes: &[u8]) {
        let Some(key) = appearance::parse_key(bytes) else { return };
        let outcome = match &mut self.panel {
            Some(Panel::Picker(p)) => p.handle(key),
            Some(Panel::Editor(e)) => e.handle(key),
            None => return,
        };
        self.apply(outcome);
    }

    pub fn action(&mut self, act: &ui::UiAction) {
        let outcome = match (&mut self.panel, act) {
            (_, ui::UiAction::OpenAppearance) => {
                self.open_picker();
                return;
            }
            (Some(Panel::Picker(p)), ui::UiAction::PickTheme(i)) => p.select(*i),
            (Some(Panel::Picker(p)), ui::UiAction::SetMode(m)) => appearance::parse_mode(m).map(|m| p.set_mode(m)).unwrap_or(Outcome::Continue),
            (Some(Panel::Picker(_)), ui::UiAction::EditTheme) => Outcome::OpenEditor,
            (Some(Panel::Editor(e)), ui::UiAction::FocusField(i)) => {
                e.focus_field(*i);
                Outcome::Continue
            }
            (Some(Panel::Picker(p)), ui::UiAction::SavePanel) => p.handle(EditKey::Save),
            (Some(Panel::Editor(e)), ui::UiAction::SavePanel) => e.handle(EditKey::Save),
            (Some(_), ui::UiAction::ClosePanel) => Outcome::Close,
            (_, ui::UiAction::FontSize(d)) => {
                self.zoom(*d);
                return;
            }
            _ => return,
        };
        self.apply(outcome);
    }

    fn apply(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Continue => {}
            Outcome::Close => {
                self.panel = None;
                self.invalidate();
            }
            Outcome::OpenEditor => self.open_editor(),
            Outcome::Preview => match &self.panel {
                Some(Panel::Picker(p)) => self.preview_colors = Some(load_theme(p.theme(), variant_for(&p.mode)).colors()),
                Some(Panel::Editor(e)) => {
                    if let Ok(r) = e.validate() {
                        self.preview_colors = Some(r.theme.colors());
                        self.preview_font = Some((r.font_size, r.font_family));
                    }
                }
                None => {}
            },
            Outcome::Save => {
                let msg = match self.panel.take() {
                    Some(Panel::Picker(p)) => {
                        let (name, mode) = (p.theme().to_string(), p.mode.clone());
                        let r = update_preferences(|pr| {
                            pr.theme = Some(name.clone());
                            pr.mode = Some(mode.clone());
                        });
                        r.map(|_| format!("Theme: {name} ({})", appearance::mode_name(&mode)))
                    }
                    Some(Panel::Editor(e)) => match e.validate() {
                        Ok(r) => {
                            self.font_size = r.font_size;
                            self.font_family = r.font_family.clone();
                            let theme_name = r.theme.name.clone();
                            save_theme(&r.theme)
                                .and_then(|_| {
                                    update_preferences(|pr| {
                                        pr.theme = Some(theme_name.clone());
                                        pr.font_size = Some(r.font_size);
                                        pr.font_family = r.font_family.clone();
                                    })
                                })
                                .map(|_| format!("Saved {} ({})", theme_name, r.theme.variant))
                        }
                        Err(err) => Ok(err),
                    },
                    None => Ok(String::new()),
                };
                self.invalidate();
                match msg {
                    Ok(m) if !m.is_empty() => self.say(m),
                    Ok(_) => {}
                    Err(e) => self.say(format!("Could not save: {e}")),
                }
            }
        }
    }

    /// Ctrl+= / Ctrl+- / Ctrl+0.
    pub fn zoom(&mut self, delta: i8) {
        let size = if delta == 0 {
            self.default_font_size
        } else {
            (self.font_size + delta as f32).clamp(appearance::MIN_FONT_SIZE, appearance::MAX_FONT_SIZE)
        };
        self.set_font_size(size);
    }

    pub fn set_font_size(&mut self, size: f32) {
        self.font_size = size;
        let _ = update_preferences(|p| p.font_size = Some(size));
        self.say(format!("Font size {}", appearance::format_size(size)));
    }

    pub fn set_font_family(&mut self, family: Option<String>) {
        self.font_family = family.clone();
        let _ = update_preferences(|p| p.font_family = family.clone());
        self.say(format!("Font: {}", family.as_deref().unwrap_or("default")));
    }

    /// `:theme [name]`
    pub fn command_theme(&mut self, arg: &str) {
        let arg = arg.trim();
        if arg.is_empty() {
            self.open_picker();
            return;
        }
        match list_themes().into_iter().find(|t| t.eq_ignore_ascii_case(arg)) {
            Some(name) => {
                let _ = update_preferences(|p| p.theme = Some(name.clone()));
                self.invalidate();
                self.say(format!("Theme: {name}"));
            }
            None => self.say(format!("No theme named '{arg}'")),
        }
    }

    /// `:mode dark|light|system`
    pub fn command_mode(&mut self, arg: &str) {
        match appearance::parse_mode(arg) {
            Some(m) => {
                let _ = update_preferences(|p| p.mode = Some(m.clone()));
                self.invalidate();
                self.say(format!("Mode: {}", appearance::mode_name(&m)));
            }
            None => self.say("Usage: :mode dark|light|system"),
        }
    }

    /// Panel layout for the renderer.
    pub fn panel_view(&self, vp: mouser_render::UiViewport) -> Option<ui::Panel> {
        match self.panel.as_ref()? {
            Panel::Picker(p) => {
                Some(ui::picker_panel(vp.width, vp.height, vp.cell_w, vp.cell_h, &p.themes, p.selected, appearance::mode_name(&p.mode)))
            }
            Panel::Editor(e) => {
                let fields: Vec<ui::EditorFieldView> = e
                    .fields
                    .iter()
                    .map(|f| ui::EditorFieldView { label: f.label.clone(), value: f.value.clone(), valid: f.is_valid(), swatch: f.swatch() })
                    .collect();
                let title = format!("Edit {} ({})", e.theme, e.variant);
                Some(ui::editor_panel(vp.width, vp.height, vp.cell_w, vp.cell_h, &title, &fields, e.focus, e.error.as_deref()))
            }
        }
    }
}
