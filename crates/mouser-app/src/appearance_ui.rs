//! Thin adapter between the core theme switcher/editor and the renderer.

use mouser_core::appearance::{self, EditKey, Outcome, ThemeEditor, ThemePicker};
use mouser_core::storage::{Preferences, ThemeMode, load_preferences, update_preferences};
use mouser_core::themes::{
    ThemeColors, active_variant, create_theme, default_theme_name, duplicate_theme, export_theme, import_theme, list_themes, load_theme,
    match_omarchy, resolve_active_theme, save_theme, valid_theme_name,
};
use mouser_mode_api::complete::{self, Engine};
use mouser_render::{Colors, ui};
use std::time::{Duration, Instant};

pub enum Panel {
    Picker(ThemePicker),
    Editor(ThemeEditor),
    /// Name prompt for New / Duplicate.
    NameForm {
        title: String,
        /// "new" or "duplicate"
        kind: &'static str,
        /// Theme being duplicated (ignored for new).
        source: String,
        name: String,
        error: Option<String>,
    },
}

/// Live appearance state: fonts, colours (with preview) and the open panel.
/// How long a status message stays in the bar.
const STATUS_TTL: Duration = Duration::from_secs(4);

pub struct Appearance {
    pub panel: Option<Panel>,
    pub font_size: f32,
    pub font_family: Option<String>,
    /// Font size from config.toml (Ctrl+0 resets to it).
    pub default_font_size: f32,
    preview_colors: Option<ThemeColors>,
    preview_font: Option<(f32, Option<String>)>,
    cache: Option<(Instant, ThemeColors)>,
    pub message: Option<(String, Instant)>,
    complete: Engine,
}

fn to_colors(c: ThemeColors) -> Colors {
    Colors { bg: c.background, fg: c.foreground, cursor: c.cursor, accent: c.accent }
}

fn variant_for(mode: &ThemeMode) -> &'static str {
    active_variant(&Preferences { mode: Some(mode.clone()), ..Default::default() })
}

impl Appearance {
    pub fn new(font_size: f32, font_family: Option<String>, default_font_size: f32) -> Self {
        Self {
            panel: None,
            font_size,
            font_family,
            default_font_size,
            preview_colors: None,
            preview_font: None,
            cache: None,
            message: None,
            complete: Engine::new(),
        }
    }

    pub fn colors(&mut self) -> Colors {
        to_colors(self.theme_colors())
    }

    /// Active theme colours including the ANSI palette (preview wins).
    pub fn theme_colors(&mut self) -> ThemeColors {
        if let Some(c) = self.preview_colors {
            return c;
        }
        match self.cache {
            Some((at, c)) if at.elapsed() < Duration::from_secs(1) => c,
            _ => {
                let c = resolve_active_theme(&load_preferences()).colors();
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
        crate::wake_after(STATUS_TTL + Duration::from_millis(50));
    }

    pub fn status(&self) -> Option<String> {
        self.message.as_ref().filter(|(_, at)| at.elapsed() < STATUS_TTL).map(|(m, _)| m.clone())
    }

    fn invalidate(&mut self) {
        self.cache = None;
        self.preview_colors = None;
        self.preview_font = None;
    }

    /// Drop the cached colours so the next frame reads the theme in preferences
    /// (an MCP `set_theme`). A colour preview in the appearance panel is cleared
    /// too, so the saved theme shows immediately; a font preview is left alone.
    pub fn reload_theme(&mut self) {
        self.cache = None;
        self.preview_colors = None;
    }

    pub fn open_picker(&mut self) {
        let prefs = load_preferences();
        let current = prefs.theme.clone().unwrap_or_else(|| default_theme_name().into());
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
        if bytes == b"\t" || bytes == b"\x1b[Z" {
            if self.try_complete(bytes == b"\x1b[Z") {
                return;
            }
        } else {
            self.complete.reset();
        }
        let Some(key) = appearance::parse_key(bytes) else { return };
        self.sync_families();
        // Keyboard peers of the switcher buttons (for demos / no mouse).
        if matches!(self.panel, Some(Panel::Picker(_))) {
            match key {
                EditKey::Char('a') => {
                    self.run_auto_theme();
                    return;
                }
                EditKey::Char('u') => {
                    self.action(&ui::UiAction::DuplicateTheme);
                    return;
                }
                EditKey::Char('n') => {
                    self.action(&ui::UiAction::NewTheme);
                    return;
                }
                _ => {}
            }
        }
        // F2: Dropper (same as the Dropper button) when a colour is focused.
        if matches!(self.panel, Some(Panel::Editor(_))) && (bytes == b"OQ" || bytes == b"[12~") {
            let ok = matches!(&self.panel, Some(Panel::Editor(e)) if e.focus_is_color());
            if ok {
                self.say("Click the screen to sample a colour…");
                crate::start_color_dropper();
            } else {
                self.say("Focus a colour field, then Dropper");
            }
            return;
        }
        match &mut self.panel {
            Some(Panel::Picker(p)) => {
                let outcome = p.handle(key);
                self.apply(outcome);
            }
            Some(Panel::Editor(e)) => {
                let outcome = e.handle(key);
                self.apply(outcome);
            }
            Some(Panel::NameForm { name, error, .. }) => match key {
                EditKey::Cancel => {
                    self.open_picker();
                }
                EditKey::Enter | EditKey::Save => {
                    let n = name.clone();
                    self.commit_name_form(&n);
                }
                EditKey::Backspace => {
                    name.pop();
                    *error = None;
                }
                EditKey::Char(c) if !c.is_control() && name.len() < 64 => {
                    name.push(c);
                    *error = None;
                }
                _ => {}
            },
            None => {}
        }
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
            (Some(Panel::Picker(_)), ui::UiAction::MatchOmarchy) => {
                match match_omarchy() {
                    Some(def) => {
                        if let Err(e) = save_theme(&def) {
                            self.say(format!("Match Omarchy: {e}"));
                            return;
                        }
                        // Also refresh the light sibling from the builtin so both variants exist.
                        let light = load_theme("Omarchy", "light");
                        let _ = save_theme(&light);
                        let _ = update_preferences(|pr| {
                            pr.theme = Some("Omarchy".into());
                        });
                        self.invalidate();
                        self.preview_colors = Some(def.colors());
                        // Keep picker open on Omarchy.
                        if let Some(Panel::Picker(p)) = &mut self.panel {
                            if let Some(i) = p.themes.iter().position(|t| t == "Omarchy") {
                                p.selected = i;
                            }
                        } else {
                            self.open_picker();
                        }
                        self.say("Matched Omarchy desktop colours");
                    }
                    None => self.say("Match Omarchy: no current Omarchy theme found"),
                }
                return;
            }
            (Some(Panel::Picker(p)), ui::UiAction::NewTheme) => {
                let source = p.theme().to_string();
                self.panel = Some(Panel::NameForm { title: "New theme".into(), kind: "new", source, name: String::new(), error: None });
                return;
            }
            (Some(Panel::Picker(p)), ui::UiAction::DuplicateTheme) => {
                let source = p.theme().to_string();
                self.panel = Some(Panel::NameForm {
                    title: format!("Duplicate '{source}'"),
                    kind: "duplicate",
                    source: source.clone(),
                    name: format!("{source} copy"),
                    error: None,
                });
                return;
            }
            (Some(Panel::Picker(p)), ui::UiAction::ExportTheme) => {
                let name = p.theme().to_string();
                let dir = export_dir_for(&name);
                match export_theme(&name, &dir) {
                    Ok(()) => self.say(format!("Exported to {}", dir.display())),
                    Err(e) => self.say(format!("Export failed: {e}")),
                }
                return;
            }
            (Some(Panel::NameForm { name, .. }), ui::UiAction::FocusField(_)) => {
                // single field; already focused
                let _ = name;
                return;
            }
            (Some(Panel::NameForm { name, .. }), ui::UiAction::SavePanel) => {
                let n = name.clone();
                self.commit_name_form(&n);
                return;
            }
            (Some(Panel::NameForm { .. }), ui::UiAction::ClosePanel) => {
                self.open_picker();
                return;
            }
            (Some(Panel::Editor(e)), ui::UiAction::FocusField(i)) => {
                e.focus_field(*i);
                Outcome::Continue
            }
            (Some(Panel::Editor(e)), ui::UiAction::Dropper) => {
                if !e.focus_is_color() {
                    self.say("Focus a colour field, then Dropper");
                    return;
                }
                self.say("Click the screen to sample a colour…");
                crate::start_color_dropper();
                return;
            }
            (Some(Panel::Picker(_)), ui::UiAction::AutoTheme) => {
                self.run_auto_theme();
                return;
            }
            (Some(Panel::Editor(e)), ui::UiAction::PickFont(i)) => e.pick_font(*i),
            (Some(Panel::Editor(e)), ui::UiAction::ScrollPanel(d)) => {
                e.scroll_fonts(*d as isize);
                Outcome::Continue
            }
            // A click on empty panel space closes the font list.
            (Some(Panel::Editor(e)), ui::UiAction::None) => {
                e.fonts.open = false;
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
            Outcome::FontPicked => {
                if let Some(Panel::Editor(e)) = &self.panel {
                    let fam = e.fields[appearance::FONT_FAMILY].value.trim().to_string();
                    let size = self.preview_font.as_ref().map(|p| p.0).unwrap_or(self.font_size);
                    // The renderer resolves and loads it on a worker thread.
                    self.preview_font = Some((size, (!fam.is_empty()).then_some(fam)));
                }
            }
            Outcome::Preview => match &self.panel {
                Some(Panel::Picker(p)) => self.preview_colors = Some(load_theme(p.theme(), variant_for(&p.mode)).colors()),
                Some(Panel::Editor(e)) => {
                    if let Ok(r) = e.validate() {
                        self.preview_colors = Some(r.theme.colors());
                        // Live preview covers colours and size only; the family is resolved and
                        // loaded once, on save, so typing in the field stays cheap.
                        let family = self.preview_font.take().map(|p| p.1).unwrap_or_else(|| self.font_family.clone());
                        self.preview_font = Some((r.font_size, family));
                    }
                }
                Some(Panel::NameForm { .. }) | None => {}
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
                    Some(Panel::NameForm { .. }) | None => Ok(String::new()),
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

    /// Hand the background font scan's result to an open editor.
    fn sync_families(&mut self) {
        if let (Some(Panel::Editor(e)), Some(f)) = (&mut self.panel, crate::font_families())
            && !e.fonts.loaded
        {
            e.set_families(f);
        }
        self.poll_dropper();
    }

    fn poll_dropper(&mut self) {
        let Some(result) = crate::take_dropper_result() else { return };
        match result {
            Ok(rgb) => {
                let Some(Panel::Editor(e)) = &mut self.panel else {
                    self.say(format!("Sampled #{rgb:06X}"));
                    return;
                };
                if !e.focus_is_color() {
                    // Prefer the first colour field if focus moved off a colour.
                    e.focus_field(appearance::FG);
                }
                let out = e.apply_color(rgb);
                self.say(format!("Dropper #{rgb:06X}"));
                self.apply(out);
            }
            Err(e) => self.say(e),
        }
    }

    /// Auto: Omarchy desktop colours when present, else a note.
    fn run_auto_theme(&mut self) {
        match match_omarchy() {
            Some(def) => {
                if let Err(e) = save_theme(&def) {
                    self.say(format!("Auto: {e}"));
                    return;
                }
                let light = load_theme("Omarchy", "light");
                let _ = save_theme(&light);
                let _ = update_preferences(|pr| {
                    pr.theme = Some("Omarchy".into());
                });
                self.invalidate();
                self.preview_colors = Some(def.colors());
                if let Some(Panel::Picker(p)) = &mut self.panel {
                    if let Some(i) = p.themes.iter().position(|t| t == "Omarchy") {
                        p.selected = i;
                    }
                } else {
                    self.open_picker();
                }
                self.say("Auto: matched Omarchy desktop theme");
            }
            None => {
                // Broader desktop probes can land here later (GTK/KDE/macOS/Windows).
                self.say("Auto: no desktop theme detected (Omarchy colors.toml missing)");
            }
        }
    }

    /// `:font` with no argument: open the editor on the font field.
    pub fn open_font_picker(&mut self) {
        self.open_picker();
        self.open_editor();
        if let Some(Panel::Editor(e)) = &mut self.panel {
            e.focus_field(appearance::FONT_FAMILY);
        }
        self.sync_families();
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
    /// Tab-complete the focused text field. False: let Tab/Shift+Tab move fields.
    fn try_complete(&mut self, reverse: bool) -> bool {
        if matches!(&self.panel, Some(Panel::NameForm { .. })) {
            let name = match &self.panel {
                Some(Panel::NameForm { name, .. }) => name.clone(),
                _ => return false,
            };
            let themes = list_themes();
            let (stem, token, matches) = complete::value_matches(&name, &themes);
            let out = self.complete.tab(&name, &stem, &token, &matches, reverse);
            if let Some(Panel::NameForm { name, error, .. }) = &mut self.panel {
                *name = out.text.clone();
                *error = None;
            }
            if let Some(msg) = out.message {
                self.say(msg);
            } else if !out.candidates.is_empty() {
                self.say(out.list_hint(80));
            }
            return true;
        }
        if !matches!(&self.panel, Some(Panel::Editor(_))) {
            return false;
        }
        let (i, current, stem, token, matches) = {
            let Some(Panel::Editor(e)) = &self.panel else { return false };
            let i = e.focus;
            let current = e.fields.get(i).map(|f| f.value.clone()).unwrap_or_default();
            let triple = if i == appearance::FONT_FAMILY {
                let names: Vec<String> = e.fonts.families.iter().cloned().collect();
                complete::value_matches(&current, &names)
            } else if i == appearance::FONT_SIZE {
                let sizes: Vec<String> =
                    ["6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16", "18", "20", "22", "24", "28", "32", "36", "42", "48"]
                        .into_iter()
                        .map(str::to_string)
                        .collect();
                complete::value_matches(&current, &sizes)
            } else {
                let values: Vec<String> = e.fields.iter().map(|f| f.value.clone()).filter(|v| !v.is_empty()).collect();
                complete::value_matches(&current, &values)
            };
            (i, current, triple.0, triple.1, triple.2)
        };
        if matches.is_empty() {
            return false;
        }
        let out = self.complete.tab(&current, &stem, &token, &matches, reverse);
        if out.candidates.len() == 1 && out.text == current && out.message.is_none() {
            return false;
        }
        let outcome = if let Some(Panel::Editor(e)) = &mut self.panel {
            e.fields[i].value = out.text.clone();
            if i == appearance::FONT_FAMILY {
                let q = e.fields[i].value.clone();
                e.fonts.refilter(&q);
                e.fonts.open = true;
            }
            match e.validate() {
                Ok(_) => {
                    e.error = None;
                    Some(Outcome::Preview)
                }
                Err(err) => {
                    e.error = Some(err);
                    None
                }
            }
        } else {
            None
        };
        if let Some(o) = outcome {
            self.apply(o);
        }
        if let Some(msg) = out.message {
            self.say(msg);
        } else if out.candidates.len() > 1 {
            self.say(out.list_hint(80));
        }
        true
    }

    pub fn panel_view(&mut self, vp: mouser_render::UiViewport) -> Option<ui::Panel> {
        self.sync_families();
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
                let dropdown = (e.focus == appearance::FONT_FAMILY && e.fonts.open).then(|| ui::FontDropdownView {
                    rows: e.fonts.visible().map(|(i, n, p)| (i, n.to_string(), p.to_vec())).collect(),
                    selected: e.fonts.selected,
                    counts: e.fonts.loaded.then(|| (e.fonts.matches.len(), e.fonts.families.len())),
                });
                Some({
                    let preview = {
                        let hex = |i: usize| fields.get(i).and_then(|f| f.swatch);
                        let fg = hex(0).unwrap_or(0xE6FBFA);
                        let bg = hex(1).unwrap_or(0x04192A);
                        let cursor = hex(2).unwrap_or(fg);
                        let accent = hex(3).unwrap_or(0x2FD8E5);
                        Some((fg, bg, cursor, accent))
                    };
                    ui::editor_panel(
                        vp.width,
                        vp.height,
                        vp.cell_w,
                        vp.cell_h,
                        &title,
                        &fields,
                        e.focus,
                        e.error.as_deref(),
                        dropdown.as_ref(),
                        preview,
                    )
                })
            }
            Panel::NameForm { title, name, error, .. } => {
                let fields = [ui::EditorFieldView {
                    label: "Name".into(),
                    value: name.clone(),
                    valid: error.is_none() && (name.is_empty() || valid_theme_name(name)),
                    swatch: None,
                }];
                Some(ui::form_panel(
                    vp.width,
                    vp.height,
                    vp.cell_w,
                    vp.cell_h,
                    title,
                    &fields,
                    0,
                    Some("Letters, numbers, spaces. Saved under ~/.mouser/themes/."),
                    error.as_deref(),
                    true,
                ))
            }
        }
    }

    fn commit_name_form(&mut self, name: &str) {
        let Some(Panel::NameForm { kind, source, .. }) = &self.panel else { return };
        let kind = *kind;
        let source = source.clone();
        let result = if kind == "new" { create_theme(name) } else { duplicate_theme(&source, name) };
        match result {
            Ok(()) => {
                let _ = update_preferences(|pr| pr.theme = Some(name.trim().into()));
                self.invalidate();
                self.open_picker();
                // Select the new theme
                if let Some(Panel::Picker(p)) = &mut self.panel
                    && let Some(i) = p.themes.iter().position(|t| t == name.trim())
                {
                    p.selected = i;
                }
                self.preview_colors = Some(load_theme(name.trim(), "dark").colors());
                self.say(format!("Theme '{name}' ready — edit colours when you like"));
            }
            Err(e) => {
                if let Some(Panel::NameForm { error, .. }) = &mut self.panel {
                    *error = Some(e);
                }
            }
        }
    }

    /// `:import-theme <dir>` — load dark.toml/light.toml from a folder.
    pub fn import_theme_cmd(&mut self, arg: &str) {
        let path = std::path::PathBuf::from(arg.trim());
        match import_theme(&path, None) {
            Ok(name) => {
                let _ = update_preferences(|pr| pr.theme = Some(name.clone()));
                self.invalidate();
                self.say(format!("Imported theme '{name}'"));
            }
            Err(e) => self.say(format!("Import failed: {e}")),
        }
    }

    /// `:export-theme [name]` — write dark/light toml under Downloads.
    pub fn export_theme_cmd(&mut self, arg: &str) {
        let prefs = load_preferences();
        let name = arg.trim();
        let name = if name.is_empty() { prefs.theme.clone().unwrap_or_else(|| default_theme_name().into()) } else { name.to_string() };
        let dir = export_dir_for(&name);
        match export_theme(&name, &dir) {
            Ok(()) => self.say(format!("Exported to {}", dir.display())),
            Err(e) => self.say(format!("Export failed: {e}")),
        }
    }
}

fn export_dir_for(name: &str) -> std::path::PathBuf {
    let slug: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let base = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from("."));
    let downloads = base.join("Downloads");
    if downloads.is_dir() { downloads.join(format!("mouser-theme-{slug}")) } else { base.join(format!("mouser-theme-{slug}")) }
}
