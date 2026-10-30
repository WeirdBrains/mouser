//! Turns terminal cells into themed paint for the renderer.

use mouser_core::Color;
use mouser_core::grid::{Cell, attr};
use mouser_core::parser::TerminalParser;
use mouser_core::themes::ThemeColors;
use mouser_render::CellPaint;
use mouser_render::frame::paint;

fn rgb(c: mouser_core::Rgb) -> u32 {
    (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32
}

fn to_rgb(v: u32) -> mouser_core::Rgb {
    mouser_core::Rgb { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8 }
}

pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = (((a >> s) & 0xff) as f32, ((b >> s) & 0xff) as f32);
        ((x + (y - x) * t).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

/// Tell the parser about the theme so OSC colour queries answer with it.
pub fn sync_theme<W: std::io::Write>(p: &mut TerminalParser<W>, t: &ThemeColors) {
    p.theme.fg = to_rgb(t.foreground);
    p.theme.bg = to_rgb(t.background);
    p.theme.cursor = to_rgb(t.cursor.unwrap_or(t.foreground));
    for (i, c) in t.palette.iter().enumerate() {
        p.theme.ansi[i] = to_rgb(*c);
    }
}

/// Resolve one cell against the theme and the parser's OSC overrides.
pub fn cell_paint<W: std::io::Write>(c: &Cell, p: &TerminalParser<W>) -> CellPaint {
    let st = c.style;
    let bold = st.attrs & attr::BOLD != 0;
    let color = |col: Color, default_slot: u16| -> Option<u32> {
        match col {
            Color::Default => (default_slot == 256).then(|| rgb(p.color_for(256))),
            // Bold brightens the 8 basic colours, like xterm's boldColors.
            Color::Indexed(i) if bold && i < 8 && default_slot == 256 => Some(rgb(p.color_for(i as u16 + 8))),
            Color::Indexed(i) => Some(rgb(p.color_for(i as u16))),
            Color::Rgb(r, g, b) => Some((r as u32) << 16 | (g as u32) << 8 | b as u32),
        }
    };
    let mut fg = color(st.fg, 256).unwrap_or(0);
    let mut bg = color(st.bg, 257);
    let window_bg = rgb(p.color_for(257));
    if st.attrs & attr::INVERSE != 0 {
        let b = bg.unwrap_or(window_bg);
        bg = Some(fg);
        fg = b;
    }
    if st.attrs & attr::DIM != 0 {
        fg = mix(fg, bg.unwrap_or(window_bg), 0.45);
    }
    let mut flags = 0;
    if bold {
        flags |= paint::BOLD;
    }
    if st.attrs & attr::STRIKE != 0 {
        flags |= paint::STRIKE;
    }
    if st.attrs & attr::OVERLINE != 0 {
        flags |= paint::OVERLINE;
    }
    if st.attrs & (attr::HIDDEN | attr::WIDE_CONT) != 0 {
        flags |= paint::NO_GLYPH;
    }
    if st.attrs & attr::WIDE != 0 {
        flags |= paint::WIDE;
    }
    let ul_color = match st.ul_color {
        Color::Default => fg,
        c => color(c, 0).unwrap_or(fg),
    };
    CellPaint { fg, bg, underline: st.underline, ul_color, flags, comb: c.comb }
}

/// Paint for every visible row of the live screen.
pub fn screen_paint<W: std::io::Write>(p: &TerminalParser<W>) -> Vec<Vec<CellPaint>> {
    (0..p.screen.size.rows).map(|y| p.screen.row(y).iter().map(|c| cell_paint(c, p)).collect()).collect()
}
