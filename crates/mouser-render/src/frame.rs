//! Pure frame composition: terminal text, cursor and the connections sidebar.
//! Kept free of windowing so it can be snapshot-tested.

use crate::text::{Draw, FontStack, luminance, mix};
use crate::ui::{self, UiLayout};
use mouser_core::kitty_graphics::PLACEHOLDER;

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
    /// Fuzzball graphics placements, lowest z first.
    pub images: &'a [ImageDraw],
    /// Per-cell colours and decorations, parallel to `lines` (missing rows or
    /// cells draw plain in `fg`).
    pub styles: &'a [Vec<CellPaint>],
    pub cursor_shape: CursorShape,
    /// Tab strip (drawn above the grid when it has 2+ tabs).
    pub tabs: Option<&'a ui::TabBar>,
}

impl Frame<'_> {
    /// Pixels above the first grid row.
    pub fn top(&self, cell_h: i32) -> i32 {
        PADDING
            + self.tabs.filter(|t| t.titles.len() > 1).map_or(0, |_| ui::tab_bar_height(cell_h))
            + self.ui.and_then(|l| l.dock.as_ref()).map_or(0, |d| d.inset)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
}

/// Paint flags for [`CellPaint`].
pub mod paint {
    pub const BOLD: u8 = 1;
    pub const STRIKE: u8 = 2;
    pub const OVERLINE: u8 = 4;
    /// Glyph is hidden (SGR 8) or the right half of a wide char.
    pub const NO_GLYPH: u8 = 8;
    pub const WIDE: u8 = 16;
    pub const ITALIC: u8 = 32;
}

/// Resolved paint for one cell (colours already themed, inverse/dim applied).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CellPaint {
    pub fg: u32,
    /// Background fill, or `None` for the window background.
    pub bg: Option<u32>,
    /// 0 none, 1 single, 2 double, 3 curly, 4 dotted, 5 dashed.
    pub underline: u8,
    pub ul_color: u32,
    pub flags: u8,
    /// Combining marks drawn over the base glyph (`'\0'` = none).
    pub comb: [char; 3],
}

impl CellPaint {
    pub fn plain(fg: u32) -> Self {
        Self { fg, bg: None, underline: 0, ul_color: fg, flags: 0, comb: ['\0'; 3] }
    }
}

fn draw_underline(canvas: &mut dyn Draw, x: i32, y: i32, w: i32, style: u8, color: u32) {
    match style {
        2 => {
            canvas.fill_rect(x, y, w, 1, color);
            canvas.fill_rect(x, y + 2, w, 1, color);
        }
        3 => {
            // Curly: a 1px zigzag with a 4px period.
            for i in 0..w {
                let phase = (x + i).rem_euclid(4);
                let dy = [0, -1, 0, 1][phase as usize];
                canvas.fill_rect(x + i, y + 1 + dy, 1, 1, color);
            }
        }
        4 => {
            for i in (0..w).filter(|i| (x + i) % 2 == 0) {
                canvas.fill_rect(x + i, y, 1, 1, color);
            }
        }
        5 => {
            for i in (0..w).filter(|i| (x + i).rem_euclid(6) < 4) {
                canvas.fill_rect(x + i, y, 1, 1, color);
            }
        }
        _ => canvas.fill_rect(x, y, w, 1, color),
    }
}

/// Cell backgrounds of a styled row. All rows' backgrounds are painted before
/// any text so wide glyphs and italics overhang cleanly, and so Kitty images
/// with `z < KITTY_BELOW_BG` can sit under them and other negative z between
/// them and the text.
fn draw_row_bgs(canvas: &mut dyn Draw, fonts: &FontStack, top: i32, styles: &[CellPaint]) {
    let (cw, ch) = (fonts.cell_w, fonts.cell_h);
    for (col, p) in styles.iter().enumerate() {
        if let Some(bg) = p.bg {
            canvas.fill_rect(PADDING + col as i32 * cw, top, cw, ch, bg);
        }
    }
}

fn draw_styled_row(canvas: &mut dyn Draw, fonts: &mut FontStack, top: i32, line: &str, styles: &[CellPaint], default_fg: u32) {
    let (cw, ch) = (fonts.cell_w, fonts.cell_h);
    let under_y = top + (fonts.ascent + 2).min(ch - 3);
    for (col, c) in line.chars().enumerate() {
        let x = PADDING + col as i32 * cw;
        let p = styles.get(col).copied().unwrap_or(CellPaint::plain(default_fg));
        let w = if p.flags & paint::WIDE != 0 { 2 * cw } else { cw };
        if p.flags & paint::NO_GLYPH == 0 && c != PLACEHOLDER {
            let g = if p.flags & paint::ITALIC != 0 { crate::text::italic_key(c) } else { c };
            canvas.draw_char(fonts, x, top, g, p.fg);
            if p.flags & paint::BOLD != 0 {
                canvas.draw_char(fonts, x + 1, top, g, p.fg);
            }
            for m in p.comb.into_iter().filter(|m| *m != '\0') {
                // Marks have zero advance and a negative bearing meant for the
                // previous pen position; centre them on the base cell instead.
                let mx = fonts.mark_cell_x(m, x, w);
                canvas.draw_char(fonts, mx, top, m, p.fg);
            }
        }
        if p.underline > 0 {
            draw_underline(canvas, x, under_y, w, p.underline, p.ul_color);
        }
        if p.flags & paint::STRIKE != 0 {
            canvas.fill_rect(x, top + ch / 2, w, 1, p.fg);
        }
        if p.flags & paint::OVERLINE != 0 {
            canvas.fill_rect(x, top, w, 1, p.fg);
        }
    }
}

/// One image placement to draw, anchored to a grid cell.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImageDraw {
    /// Unique per image upload (texture cache key).
    pub key: u64,
    pub rgba: std::sync::Arc<Vec<u8>>,
    pub img_w: u32,
    pub img_h: u32,
    /// Source rectangle (x, y, w, h) in image pixels.
    pub src: (u32, u32, u32, u32),
    /// Top-left cell (may be negative when partly scrolled off) and pixel offset.
    pub col: i32,
    pub row: i32,
    pub x_off: i32,
    pub y_off: i32,
    /// Displayed size in pixels.
    pub width: u32,
    pub height: u32,
    /// Negative: under the text; otherwise over it.
    pub z: i32,
}

/// Fuzzball placements on screen `alt` visible in absolute rows `top .. top + rows`,
/// as draw commands (row relative to `top`) sized for `cell` pixels, lowest z first.
/// Downscaled images come from `scaler`'s cache.
pub fn image_draws(
    gfx: &mouser_core::graphics::Graphics,
    alt: bool,
    top: i64,
    rows: usize,
    cell: (u32, u32),
    scaler: &mut crate::scale::ImageScaler,
) -> Vec<ImageDraw> {
    layered_image_draws(&[(gfx, gfx.draws(alt, top, rows, cell))], scaler)
}

/// Draw commands for several stores, each with its draws (for example Kitty
/// placements and placeholders, then Fuzzball placements). Sorted by z; at
/// equal z later layers draw on top, so Fuzzball goes last to take precedence.
pub fn layered_image_draws(
    layers: &[(&mouser_core::graphics::Graphics, Vec<mouser_core::graphics::Draw>)],
    scaler: &mut crate::scale::ImageScaler,
) -> Vec<ImageDraw> {
    let mut draws: Vec<ImageDraw> = Vec::new();
    for (gfx, list) in layers {
        for d in list {
            let Some(im) = gfx.images.get(&d.image_id) else { continue };
            let i32c = |v: i64| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
            draws.push(ImageDraw {
                key: im.key,
                rgba: im.rgba.clone(),
                img_w: im.width,
                img_h: im.height,
                src: d.src,
                col: i32c(d.col),
                row: i32c(d.row),
                x_off: i32c(d.x_off),
                y_off: i32c(d.y_off),
                width: d.width,
                height: d.height,
                z: d.z,
            });
        }
    }
    draws.sort_by_key(|d| d.z);
    scaler.prepare(&mut draws);
    draws
}

/// Kitty images with a z-index below this draw under cell backgrounds.
pub const KITTY_BELOW_BG: i32 = -1_073_741_824;

fn draw_images(canvas: &mut dyn Draw, fonts: &FontStack, images: &[ImageDraw], z: std::ops::Range<i64>, top: i32) {
    for img in images.iter().filter(|i| z.contains(&(i.z as i64))) {
        let x = PADDING + img.col * fonts.cell_w + img.x_off;
        let y = top + img.row * fonts.cell_h + img.y_off;
        canvas.draw_image(img, x, y);
    }
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
    // The commander view goes first: its previews are images, drawn with the rest.
    if let Some(d) = frame.ui.and_then(|l| l.dock.as_ref()) {
        let accent = frame.accent.unwrap_or(accent_for(frame.bg));
        let (bg, fg) = (frame.bg, frame.fg);
        let dark = luminance(bg) < 128.0;
        let panel = if dark { mix(bg, 0xFFFFFF, 0.05) } else { mix(bg, 0x000000, 0.04) };
        draw_panel(canvas, fonts, &d.panel, bg, fg, panel, mix(bg, fg, 0.25), mix(panel, fg, 0.65), accent);
    }
    let (cw, ch) = (fonts.cell_w, fonts.cell_h);
    let top0 = frame.top(ch);
    let (lo, below_bg) = (i64::MIN, KITTY_BELOW_BG as i64);
    draw_images(canvas, fonts, frame.images, lo..below_bg, top0);
    for (row, st) in frame.styles.iter().enumerate().take(frame.lines.len()) {
        draw_row_bgs(canvas, fonts, top0 + row as i32 * ch, st);
    }
    draw_images(canvas, fonts, frame.images, below_bg..0, top0);
    for (row, line) in frame.lines.iter().enumerate() {
        let top = top0 + row as i32 * ch;
        match frame.styles.get(row) {
            Some(st) if !st.is_empty() => draw_styled_row(canvas, fonts, top, line, st, frame.fg),
            _ if line.contains(PLACEHOLDER) => canvas.draw_text(fonts, PADDING, top, &line.replace(PLACEHOLDER, " "), frame.fg),
            _ => canvas.draw_text(fonts, PADDING, top, line, frame.fg),
        }
    }
    draw_images(canvas, fonts, frame.images, 0..i64::MAX, top0);
    if let Some((cx, cy)) = frame.cursor {
        let x = PADDING + cx as i32 * cw;
        let y = top0 + cy as i32 * ch;
        let color = frame.cursor_color.unwrap_or(mix(frame.bg, frame.fg, 0.85));
        let wide = frame.styles.get(cy).and_then(|r| r.get(cx)).is_some_and(|p| p.flags & paint::WIDE != 0);
        let w = if wide { 2 * cw } else { cw };
        match frame.cursor_shape {
            CursorShape::Block => {
                canvas.fill_rect(x, y, w, ch, color);
                if let Some(c) = frame.lines.get(cy).and_then(|l| l.chars().nth(cx)).filter(|c| *c != PLACEHOLDER) {
                    canvas.draw_char(fonts, x, y, c, frame.bg);
                }
            }
            CursorShape::Underline => canvas.fill_rect(x, y + ch - 2, w, 2, color),
            CursorShape::Bar => canvas.fill_rect(x, y, 2, ch, color),
        }
    }
    if let Some(bar) = frame.tabs.filter(|t| t.titles.len() > 1) {
        let w = frame.ui.map(|l| l.sidebar_rect.x).unwrap_or(canvas.width() as i32);
        draw_tabs(canvas, fonts, bar, w, frame.bg, frame.fg, frame.accent.unwrap_or(accent_for(frame.bg)));
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
    // Drop shadow, then border and an opaque body.
    canvas.fill_rect(r.x + 3, r.y + 5, r.w + 2, r.h + 2, mix(bg, 0x000000, 0.55));
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
            ui::PanelStyle::Tile { selected } => {
                let (edge, t) = if selected { (accent, 2) } else { (border, 1) };
                canvas.fill_rect(b.x, b.y, b.w, b.h, edge);
                canvas.fill_rect(b.x + t, b.y + t, b.w - 2 * t, b.h - 2 * t, mix(bg, panel, 0.5));
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
        if !layout.engines.is_empty() {
            let t = layout.engines_title;
            canvas.draw_text(fonts, t.x, t.y + (t.h - cell_h) / 2, ui::ENGINES_TITLE, fg);
            for e in &layout.engines {
                let r = e.rect;
                let line_h = cell_h + 8;
                let active = e.state == ui::EngineState::Active;
                let bg = if active { mix(panel, accent, 0.18) } else { mix(panel, fg, 0.05) };
                canvas.fill_rect(r.x, r.y, r.w, r.h, bg);
                let label = e.state.label();
                let lw = label.chars().count() as i32 * fonts.cell_w;
                let max = ((r.w - lw - 3 * ui::BUTTON_PAD) / fonts.cell_w).max(0) as usize;
                let name: String = e.name.chars().take(max).collect();
                let ty = r.y + (line_h - cell_h) / 2;
                canvas.draw_text(fonts, r.x + ui::BUTTON_PAD, ty, &name, if active { accent } else { fg });
                let state_col = match e.state {
                    ui::EngineState::Active => accent,
                    ui::EngineState::Ok => muted,
                    _ => mix(muted, 0xF7768E, 0.6),
                };
                canvas.draw_text(fonts, r.x + r.w - ui::BUTTON_PAD - lw, ty, label, state_col);
                if layout.engine_drag.is_some_and(|d| layout.engines.get(d.from) == Some(e)) {
                    // The dragged row stays in place, outlined, under its ghost.
                    for (x, y, w, h) in [(r.x, r.y, r.w, 1), (r.x, r.y + r.h - 1, r.w, 1), (r.x, r.y, 1, r.h), (r.x + r.w - 1, r.y, 1, r.h)] {
                        canvas.fill_rect(x, y, w, h, accent);
                    }
                }
                if let Some(n) = &e.note {
                    let max = ((r.w - 2 * ui::BUTTON_PAD) / fonts.cell_w).max(0) as usize;
                    let t: String = n.chars().take(max).collect();
                    canvas.draw_text(fonts, r.x + ui::BUTTON_PAD, ty + line_h - 6, &t, muted);
                }
            }
        }
        if let Some(d) = layout.engine_drag
            && let Some(row) = layout.engines.get(d.from)
        {
            // Ghost of the row under the pointer.
            let line_h = cell_h + 8;
            let gy = d.pos.1 - line_h / 2;
            let r = row.rect;
            canvas.fill_rect(r.x, gy, r.w, line_h, mix(panel, accent, 0.35));
            canvas.draw_text(fonts, r.x + ui::BUTTON_PAD, gy + (line_h - cell_h) / 2, &row.name, fg);
            // Insertion line between rows, drawn over the ghost.
            if let Some(at) = d.target(layout) {
                // Insertion line between rows.
                let y = match layout.engines.get(at) {
                    Some(e) => e.rect.y - 2,
                    None => layout.engines.last().map_or(0, |e| e.rect.y + e.rect.h),
                };
                let r = row.rect;
                canvas.fill_rect(r.x - 2, y - 1, r.w + 4, 3, accent);
                canvas.fill_rect(r.x - 4, y - 3, 3, 7, accent);
                canvas.fill_rect(r.x + r.w + 1, y - 3, 3, 7, accent);
            }
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

fn draw_tabs(canvas: &mut dyn Draw, fonts: &mut FontStack, bar: &ui::TabBar, width: i32, bg: u32, fg: u32, accent: u32) {
    let h = ui::tab_bar_height(fonts.cell_h);
    let strip = mix(bg, fg, 0.06);
    canvas.fill_rect(0, 0, width, h, strip);
    canvas.fill_rect(0, h - 1, width, 1, mix(bg, fg, 0.2));
    for (i, (r, close)) in ui::tab_rects(bar.titles.len(), width, fonts.cell_w, fonts.cell_h).into_iter().enumerate() {
        let active = i == bar.active;
        if active {
            canvas.fill_rect(r.x, r.y, r.w, r.h, bg);
            canvas.fill_rect(r.x, r.h - 2, r.w, 2, accent);
        }
        canvas.fill_rect(r.x + r.w - 1, 4, 1, r.h - 8, mix(bg, fg, 0.2));
        let text = if active { fg } else { mix(strip, fg, 0.6) };
        let max = ((close.x - r.x) / fonts.cell_w - 2).max(0) as usize;
        let t: String = bar.titles[i].chars().take(max).collect();
        let ty = (r.h - fonts.cell_h) / 2;
        canvas.draw_text(fonts, r.x + fonts.cell_w, ty, &t, text);
        canvas.draw_char(fonts, close.x + fonts.cell_w / 2, ty, '×', mix(strip, fg, 0.5));
    }
}
