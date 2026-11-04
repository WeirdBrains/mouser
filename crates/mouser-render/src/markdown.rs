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
        out.push(PanelItem {
            rect: Rect { x: cx, y, w: sw.min(right - cx), h: row_h },
            text: s.text.clone(),
            style: PanelStyle::Md { flags, heading },
            action,
        });
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
