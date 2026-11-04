//! Markdown rows (`mouser_core::markdown`) as panel items: commander
//! previews and the chat transcript (docs/markdown.md). The frame draws the
//! styles ([`PanelStyle::Md`] and friends) in the theme's colours.

use crate::ui::{PanelItem, PanelStyle, Rect, UiAction};
use mouser_core::markdown::{Kind, LINK, Line, width};

/// Items for one row at (`x`, `y`), `w` px wide. `prev_code` / `next_code`:
/// whether the rows around it are code too (one rounded box per block).
/// `link` makes the click action of a link span.
#[allow(clippy::too_many_arguments)]
pub fn line_items(
    l: &Line,
    (prev_code, next_code): (bool, bool),
    x: i32,
    y: i32,
    (cw, row_h): (i32, i32),
    w: i32,
    link: &mut dyn FnMut(&str) -> UiAction,
    out: &mut Vec<PanelItem>,
) {
    let it = |rect, text: String, style| PanelItem { rect, text, style, action: UiAction::None };
    for q in 0..l.quote as i32 {
        out.push(it(Rect { x: x + q * 2 * cw + cw / 3, y, w: 3, h: row_h }, String::new(), PanelStyle::MdQuote));
    }
    let sx = x + l.start() as i32 * cw;
    let right = x + w;
    match l.kind {
        Kind::Rule => {
            out.push(it(Rect { x: sx, y, w: right - sx, h: row_h }, String::new(), PanelStyle::MdRule));
            return;
        }
        Kind::Code => {
            out.push(it(
                Rect { x: sx - cw / 2, y, w: right - sx + cw / 2, h: row_h },
                String::new(),
                PanelStyle::MdCode { top: !prev_code, bottom: !next_code },
            ));
        }
        _ => {}
    }
    let heading = if let Kind::Heading(n) = l.kind { n } else { 0 };
    let mut cx = sx;
    for s in &l.spans {
        if cx >= right {
            break;
        }
        let sw = width(&s.text) as i32 * cw;
        // Code block text is plain on the block's background.
        let flags = if l.kind == Kind::Code { 0 } else { s.flags };
        let action = match (&s.link, s.flags & LINK != 0) {
            (Some(u), true) => link(u),
            _ => UiAction::None,
        };
        let style = if l.kind == Kind::Code && s.hl != 0 { PanelStyle::MdToken(s.hl) } else { PanelStyle::Md { flags, heading } };
        out.push(PanelItem { rect: Rect { x: cx, y, w: sw.min(right - cx), h: row_h }, text: s.text.clone(), style, action });
        cx += sw;
    }
}

/// Items for `lines` from (`x`, `y`), one row each `row_h` px, at most
/// `rows` rows from `skip`. Returns the rows used.
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
    let code = |i: usize| lines.get(i).is_some_and(|l| l.kind == Kind::Code);
    let mut n = 0;
    for (i, l) in lines.iter().enumerate().skip(skip).take(rows) {
        let around = (i > skip && code(i - 1), i + 1 < skip + rows && code(i + 1));
        line_items(l, around, x, y + n as i32 * cell.1, cell, w, link, out);
        n += 1;
    }
    n
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
