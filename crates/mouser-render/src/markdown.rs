//! Markdown rows (`mouser_core::markdown`) as panel items: commander
//! previews and the chat transcript (docs/markdown.md). The frame draws the
//! styles ([`PanelStyle::Md`] and friends) in the theme's colours.

use crate::ui::{PanelItem, UiAction};
use mouser_core::markdown::Line;

/// Items for one row; see [`mouser_mode_api::markdown::line_items`].
#[allow(clippy::too_many_arguments)]
pub fn line_items(
    l: &Line,
    around: (bool, bool),
    x: i32,
    y: i32,
    cell: (i32, i32),
    w: i32,
    link: &mut dyn FnMut(&str) -> UiAction,
    out: &mut Vec<PanelItem>,
) {
    mouser_mode_api::markdown::line_items(l, around, x, y, cell, w, link, out)
}

/// Items for `lines`; see [`mouser_mode_api::markdown::items`].
#[allow(clippy::too_many_arguments)]
pub fn items(
    lines: &[Line],
    skip: usize,
    rows: usize,
    x: i32,
    y: i32,
    cell: (i32, i32),
    w: i32,
    link: &mut dyn FnMut(&str) -> UiAction,
    out: &mut Vec<PanelItem>,
) -> usize {
    mouser_mode_api::markdown::items(lines, skip, rows, x, y, cell, w, link, out)
}

static PALETTE: std::sync::RwLock<Option<[u32; 16]>> = std::sync::RwLock::new(None);

/// The active theme's ANSI palette, for code token colours (the app sets it
/// whenever it reads the theme; light and dark themes bring their own).
pub fn set_palette(p: [u32; 16]) {
    if let Ok(mut g) = PALETTE.write() {
        *g = Some(p);
    }
}

/// The palette set by [`set_palette`], else Mouser's default one.
pub fn palette() -> [u32; 16] {
    if let Some(p) = PALETTE.read().ok().and_then(|g| *g) {
        return p;
    }
    let mut p = [0u32; 16];
    for (i, h) in mouser_core::themes::DEFAULT_PALETTE.iter().enumerate() {
        p[i] = u32::from_str_radix(h.trim_start_matches('#'), 16).unwrap_or(0xC0C0C0);
    }
    p
}

/// The colour of a code token class (`mouser_core::highlight`): a palette
/// slot of the theme; comments are muted text. `fg` / `panel`: the panel's
/// text and background, for the muted mix and dark/light choice.
pub fn token_color(hl: u8, fg: u32, panel: u32) -> u32 {
    use mouser_core::highlight as h;
    let p = palette();
    let dark = luma(panel) < 0.5;
    // Dark themes read better with the bright slots.
    let slot = |n: usize| if dark { p[n + 8] } else { p[n] };
    match hl {
        h::KEYWORD => slot(5),
        h::TYPE => slot(3),
        h::STRING | h::ADDED => slot(2),
        h::NUMBER | h::META => slot(6),
        h::FUNC => slot(4),
        h::REMOVED => slot(1),
        h::COMMENT => mix(panel, fg, 0.55),
        _ => fg,
    }
}

fn luma(c: u32) -> f32 {
    let (r, g, b) = ((c >> 16) & 255, (c >> 8) & 255, c & 255);
    (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0
}

fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = (((a >> s) & 255) as f32, ((b >> s) & 255) as f32);
        ((x + (y - x) * t).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}
