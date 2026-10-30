//! Pure frame composition: terminal text, cursor and the connections sidebar.
//! Kept free of windowing so it can be snapshot-tested.

use crate::text::{Canvas, FontStack, luminance, mix};
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

pub fn render_frame(canvas: &mut Canvas, fonts: &mut FontStack, frame: &Frame) {
    canvas.clear(frame.bg);
    let (cw, ch) = (fonts.cell_w, fonts.cell_h);
    for (row, line) in frame.lines.iter().enumerate() {
        let top = PADDING + row as i32 * ch;
        canvas.draw_text(fonts, PADDING, top, line, frame.fg);
    }
    if let Some((cx, cy)) = frame.cursor {
        let x = PADDING + cx as i32 * cw;
        let y = PADDING + cy as i32 * ch;
        canvas.fill_rect(x, y, cw, ch, mix(frame.bg, frame.fg, 0.85));
        if let Some(c) = frame.lines.get(cy).and_then(|l| l.chars().nth(cx)) {
            canvas.draw_char(fonts, x, y, c, frame.bg);
        }
    }
    if let Some(layout) = frame.ui {
        draw_ui(canvas, fonts, layout, frame.bg, frame.fg);
    }
    if let Some(status) = frame.status {
        let h = ch + 8;
        let y = canvas.height as i32 - h;
        let w = frame.ui.map(|l| l.sidebar_rect.x).unwrap_or(canvas.width as i32);
        canvas.fill_rect(0, y, w, h, mix(frame.bg, frame.fg, 0.12));
        canvas.fill_rect(0, y, w, 1, accent_for(frame.bg));
        canvas.draw_text(fonts, PADDING, y + 4, status, frame.fg);
    }
}

fn draw_button(canvas: &mut Canvas, fonts: &mut FontStack, r: ui::Rect, label: &str, fill: u32, text: u32) {
    canvas.fill_rect(r.x, r.y, r.w, r.h, fill);
    let top = r.y + (r.h - fonts.cell_h) / 2;
    canvas.draw_text(fonts, r.x + ui::BUTTON_PAD, top, label, text);
}

fn draw_ui(canvas: &mut Canvas, fonts: &mut FontStack, layout: &UiLayout, bg: u32, fg: u32) {
    let dark = luminance(bg) < 128.0;
    let panel = if dark { mix(bg, 0xFFFFFF, 0.05) } else { mix(bg, 0x000000, 0.04) };
    let border = mix(bg, fg, 0.25);
    let muted = mix(panel, fg, 0.65);
    let accent = accent_for(bg);
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
            draw_button(canvas, fonts, item.edit_rect, ui::EDIT_LABEL, mix(panel, fg, 0.08), muted);
            draw_button(canvas, fonts, item.delete_rect, ui::DELETE_LABEL, mix(panel, fg, 0.08), muted);
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
