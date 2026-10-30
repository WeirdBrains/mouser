//! Pure frame composition: terminal text, cursor and the connections sidebar.
//! Kept free of windowing so it can be snapshot-tested.

use crate::text::{Draw, FontStack, luminance, mix};
use crate::ui::{self, UiLayout};

/// Padding between the window edge and the terminal grid, in pixels.
pub const PADDING: i32 = 8;

pub struct Frame<'a> {
    pub lines: &'a [String],
    /// Cursor (column, row) within `lines`, if it should be drawn.
    pub cursor: Option<(usize, usize)>,
    pub bg: u32,
    pub fg: u32,
    pub ui: Option<&'a UiLayout>,
    /// Bottom status / command line.
    pub status: Option<&'a str>,
    /// Theme accent (sidebar buttons, highlights); derived from `bg` if `None`.
    pub accent: Option<u32>,
    /// Cursor colour; derived from `fg` if `None`.
    pub cursor_color: Option<u32>,
}

/// Grid size that fits a `width` x `height` pixel area.
pub fn grid_size(fonts: &FontStack, width: i32, height: i32) -> (usize, usize) {
    let cols = ((width - 2 * PADDING) / fonts.cell_w).max(1) as usize;
    let rows = ((height - 2 * PADDING) / fonts.cell_h).max(1) as usize;
    (cols, rows)
}

/// Accent colour that reads well on `bg`.
pub fn accent_for(bg: u32) -> u32 {
    if luminance(bg) < 128.0 { 0x2FD8E5 } else { 0x0B7C88 }
}

pub fn render_frame(canvas: &mut dyn Draw, fonts: &mut FontStack, frame: &Frame) {
    canvas.clear(frame.bg);
    let (cw, ch) = (fonts.cell_w, fonts.cell_h);
    for (row, line) in frame.lines.iter().enumerate() {
        let top = PADDING + row as i32 * ch;
        canvas.draw_text(fonts, PADDING, top, line, frame.fg);
    }
    if let Some((cx, cy)) = frame.cursor {
        let x = PADDING + cx as i32 * cw;
        let y = PADDING + cy as i32 * ch;
        canvas.fill_rect(x, y, cw, ch, frame.cursor_color.unwrap_or(mix(frame.bg, frame.fg, 0.85)));
        if let Some(c) = frame.lines.get(cy).and_then(|l| l.chars().nth(cx)) {
            canvas.draw_char(fonts, x, y, c, frame.bg);
        }
    }
    if let Some(layout) = frame.ui {
        let accent = frame.accent.unwrap_or(accent_for(frame.bg));
        draw_ui(canvas, fonts, layout, frame.bg, frame.fg, accent);
    }
    if let Some(status) = frame.status {
        let h = ch + 8;
        let y = canvas.height() as i32 - h;
        let w = frame.ui.map(|l| l.sidebar_rect.x).unwrap_or(canvas.width() as i32);
        canvas.fill_rect(0, y, w, h, mix(frame.bg, frame.fg, 0.12));
        canvas.fill_rect(0, y, w, 1, frame.accent.unwrap_or(accent_for(frame.bg)));
        canvas.draw_text(fonts, PADDING, y + 4, status, frame.fg);
    }
}

fn draw_button(canvas: &mut dyn Draw, fonts: &mut FontStack, r: ui::Rect, label: &str, fill: u32, text: u32) {
    canvas.fill_rect(r.x, r.y, r.w, r.h, fill);
    let top = r.y + (r.h - fonts.cell_h) / 2;
    canvas.draw_text(fonts, r.x + ui::BUTTON_PAD, top, label, text);
}

fn draw_ui(canvas: &mut dyn Draw, fonts: &mut FontStack, layout: &UiLayout, bg: u32, fg: u32, accent: u32) {
    let dark = luminance(bg) < 128.0;
    let panel = if dark { mix(bg, 0xFFFFFF, 0.05) } else { mix(bg, 0x000000, 0.04) };
    let border = mix(bg, fg, 0.25);
    let muted = mix(panel, fg, 0.65);
    let s = layout.sidebar_rect;
    if s.w > 0 {
        draw_sidebar(canvas, fonts, layout, fg, panel, border, muted, accent);
    }
    if let Some(p) = &layout.panel {
        draw_panel(canvas, fonts, p, bg, fg, panel, border, muted, accent);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_panel(canvas: &mut dyn Draw, fonts: &mut FontStack, p: &ui::Panel, bg: u32, fg: u32, panel: u32, border: u32, muted: u32, accent: u32) {
    let r = p.rect;
    canvas.fill_rect(r.x - 1, r.y - 1, r.w + 2, r.h + 2, border);
    canvas.fill_rect(r.x, r.y, r.w, r.h, panel);
    canvas.fill_rect(r.x, r.y, r.w, 2, accent);
    let ch = fonts.cell_h;
    for it in &p.items {
        let b = it.rect;
        let ty = b.y + (b.h - ch) / 2;
        let max = (b.w / fonts.cell_w).max(0) as usize;
        let clip = |t: &str, n: usize| -> String { t.chars().take(n).collect() };
        match it.style {
            ui::PanelStyle::Title => canvas.draw_text(fonts, b.x, ty, &clip(&it.text, max), fg),
            ui::PanelStyle::Label => canvas.draw_text(fonts, b.x, ty, &clip(&it.text, max), muted),
            ui::PanelStyle::Hint => canvas.draw_text(fonts, b.x, ty, &clip(&it.text, max), mix(panel, fg, 0.45)),
            ui::PanelStyle::Error => canvas.draw_text(fonts, b.x, ty, &clip(&it.text, max), 0xF7768E),
            ui::PanelStyle::Row { selected } => {
                if selected {
                    canvas.fill_rect(b.x, b.y, b.w, b.h, mix(panel, accent, 0.22));
                    canvas.fill_rect(b.x, b.y, 3, b.h, accent);
                }
                canvas.draw_text(fonts, b.x + ui::BUTTON_PAD, ty, &clip(&it.text, max.saturating_sub(1)), if selected { fg } else { muted });
            }
            ui::PanelStyle::Button { primary } => {
                let (fill, text) = if primary { (accent, bg) } else { (mix(panel, fg, 0.1), fg) };
                draw_button(canvas, fonts, b, &it.text, fill, text);
            }
            ui::PanelStyle::Field { focused, valid } => {
                let edge = if !valid {
                    0xF7768E
                } else if focused {
                    accent
                } else {
                    border
                };
                canvas.fill_rect(b.x, b.y, b.w, b.h, edge);
                canvas.fill_rect(b.x + 1, b.y + 1, b.w - 2, b.h - 2, mix(bg, panel, 0.5));
                let n = max.saturating_sub(1);
                let chars: Vec<char> = it.text.chars().collect();
                let shown: String = chars[chars.len().saturating_sub(n)..].iter().collect();
                canvas.draw_text(fonts, b.x + 4, ty, &shown, fg);
                if focused {
                    let cx = b.x + 4 + shown.chars().count() as i32 * fonts.cell_w;
                    canvas.fill_rect(cx, b.y + 3, 2, b.h - 6, accent);
                }
            }
            ui::PanelStyle::Popup => {
                canvas.fill_rect(b.x - 1, b.y - 1, b.w + 2, b.h + 2, accent);
                canvas.fill_rect(b.x, b.y, b.w, b.h, mix(bg, panel, 0.35));
            }
            ui::PanelStyle::Option { selected, highlight } => {
                if selected {
                    canvas.fill_rect(b.x, b.y, b.w, b.h, mix(panel, accent, 0.22));
                    canvas.fill_rect(b.x, b.y, 3, b.h, accent);
                }
                let base = if selected { fg } else { muted };
                for (i, c) in it.text.chars().take(max.saturating_sub(1)).enumerate() {
                    let color = if i < 64 && highlight & (1 << i) != 0 { accent } else { base };
                    canvas.draw_char(fonts, b.x + ui::BUTTON_PAD + i as i32 * fonts.cell_w, ty, c, color);
                }
            }
            ui::PanelStyle::Swatch(c) => {
                canvas.fill_rect(b.x, b.y, b.w, b.h, border);
                canvas.fill_rect(b.x + 1, b.y + 1, b.w - 2, b.h - 2, c);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_sidebar(canvas: &mut dyn Draw, fonts: &mut FontStack, layout: &UiLayout, fg: u32, panel: u32, border: u32, muted: u32, accent: u32) {
    let s = layout.sidebar_rect;
    canvas.fill_rect(s.x, s.y, s.w, s.h, panel);
    canvas.fill_rect(s.x, s.y, 1, s.h, border);
    let cell_h = fonts.cell_h;
    let c = layout.collapse_rect;
    if layout.collapsed {
        // Thin strip: an expand chevron near the top, centred.
        let x = s.x + (s.w - fonts.cell_w) / 2;
        canvas.draw_text(fonts, x, ui::SIDEBAR_MARGIN + 4, ui::EXPAND_LABEL, accent);
    } else {
        canvas.fill_rect(s.x - 1, s.y, 3, s.h, border);
        let title_y = c.y + (c.h - cell_h) / 2;
        canvas.draw_text(fonts, s.x + ui::SIDEBAR_MARGIN, title_y, ui::SIDEBAR_TITLE, muted);
        draw_button(canvas, fonts, c, ui::COLLAPSE_LABEL, mix(panel, fg, 0.08), accent);
        if let Some((r, text)) = &layout.notice {
            let max = ((r.w - 2 * ui::BUTTON_PAD) / fonts.cell_w).max(0) as usize;
            let t: String = text.chars().take(max).collect();
            draw_button(canvas, fonts, *r, &t, mix(panel, accent, 0.12), fg);
        }
        let a = layout.appearance_rect;
        if a.w > 0 {
            draw_button(canvas, fonts, a, ui::APPEARANCE_LABEL, mix(panel, accent, 0.18), accent);
        }
    }
    let max_chars = |w: i32, cw: i32| (w / cw).max(0) as usize;
    for cat in &layout.categories {
        let h = cat.header_rect;
        let title: String = ui::category_title(&cat.category).chars().take(max_chars(h.w, fonts.cell_w)).collect();
        canvas.draw_text(fonts, h.x, h.y + (h.h - cell_h) / 2, &title, fg);
        draw_button(canvas, fonts, cat.add_rect, ui::ADD_LABEL, mix(panel, accent, 0.18), accent);
        for item in &cat.items {
            let avail = item.edit_rect.x - 8 - (h.x + fonts.cell_w);
            let name: String = item.name.chars().take(max_chars(avail, fonts.cell_w)).collect();
            canvas.draw_text(fonts, h.x + fonts.cell_w, item.edit_rect.y + (item.edit_rect.h - cell_h) / 2, &name, muted);
            if item.read_only {
                draw_button(canvas, fonts, item.edit_rect, ui::INFO_LABEL, mix(panel, fg, 0.05), mix(panel, fg, 0.45));
            } else {
                draw_button(canvas, fonts, item.edit_rect, ui::EDIT_LABEL, mix(panel, fg, 0.08), muted);
                draw_button(canvas, fonts, item.delete_rect, ui::DELETE_LABEL, mix(panel, fg, 0.08), muted);
            }
        }
    }
    if let Some(modal) = &layout.modal {
        let m = modal.rect;
        canvas.fill_rect(m.x - 1, m.y - 1, m.w + 2, m.h + 2, border);
        canvas.fill_rect(m.x, m.y, m.w, m.h, panel);
        let title = match &modal.kind {
            ui::ModalKind::Add(c) => format!("Add {}", ui::category_title(c)),
            ui::ModalKind::Edit { category, name } => format!("Edit {}: {}", ui::category_title(category), name),
            ui::ModalKind::ConfirmDelete { category, name } => format!("Delete {}: {}?", ui::category_title(category), name),
        };
        canvas.draw_text(fonts, m.x + 12, m.y + 12, &title, fg);
        let adv = if modal.advanced_open { "Advanced ▾" } else { "Advanced ▸" };
        canvas.draw_text(fonts, m.x + 12, m.y + 12 + 2 * cell_h, adv, accent);
    }
}
