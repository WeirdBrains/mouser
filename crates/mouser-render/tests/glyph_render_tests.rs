//! Snapshot and geometry tests for glyph placement, using the embedded font so
//! results are identical on every machine.

use mouser_render::text::{Canvas, FontStack};
use mouser_render::ui::{self, Category};
use mouser_render::{Frame, PADDING, render_frame};
use std::collections::BTreeMap;

const BG: u32 = 0x04192A;
const FG: u32 = 0xE6FBFA;

fn render(text: &str, fonts: &mut FontStack) -> (Vec<u32>, usize, usize) {
    let w = (PADDING * 2 + fonts.cell_w * text.chars().count() as i32) as usize;
    let h = (PADDING * 2 + fonts.cell_h) as usize;
    let mut buf = vec![0u32; w * h];
    let lines = vec![text.to_string()];
    let mut canvas = Canvas::new(&mut buf, w, h);
    render_frame(&mut canvas, fonts, &Frame { lines: &lines, cursor: None, bg: BG, fg: FG, ui: None });
    (buf, w, h)
}

/// Pixel is at least ~60% covered by a glyph (ignores faint anti-aliasing).
fn solid(px: u32) -> bool {
    px & 0xFF > 0xB0
}

/// (top, bottom) solid ink rows inside cell `i`, if any.
fn ink_rows(buf: &[u32], w: usize, h: usize, fonts: &FontStack, i: usize) -> Option<(usize, usize)> {
    let x0 = PADDING as usize + i * fonts.cell_w as usize;
    let rows: Vec<usize> = (0..h).filter(|&y| (x0..x0 + fonts.cell_w as usize).any(|x| solid(buf[y * w + x]))).collect();
    Some((*rows.first()?, *rows.last()?))
}

fn ascii_snapshot(buf: &[u32], w: usize, h: usize) -> String {
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            let v = buf[y * w + x] & 0xFF; // blue channel tracks coverage for BG->FG
            out.push(if v > 0xB0 {
                '#'
            } else if v > 0x60 {
                '+'
            } else {
                '.'
            });
        }
        out.push('\n');
    }
    out
}

fn check_snapshot(name: &str, actual: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots").join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() || !path.exists() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap();
    assert_eq!(expected, actual, "snapshot {name} changed; rerun with UPDATE_SNAPSHOTS=1 if intended");
}

#[test]
fn letters_share_one_baseline() {
    let mut fonts = FontStack::embedded(16.0);
    let text = "agent_tools";
    let (buf, w, h) = render(text, &mut fonts);
    // Letters without descenders must all end on the same row.
    let bottoms: Vec<usize> =
        text.chars().enumerate().filter(|(_, c)| "aentols".contains(*c)).map(|(i, _)| ink_rows(&buf, w, h, &fonts, i).unwrap().1).collect();
    assert!(bottoms.windows(2).all(|p| p[0] == p[1]), "baselines bounce: {bottoms:?}");
    // And that row is the shared baseline.
    let baseline = PADDING as usize + fonts.ascent as usize;
    assert!(bottoms[0] + 1 == baseline || bottoms[0] == baseline, "bottom {} vs baseline {baseline}", bottoms[0]);
    // Ascender letters reach higher than x-height letters, but end on the same row.
    let (t_top, t_bot) = ink_rows(&buf, w, h, &fonts, 3).unwrap(); // 'n'
    let (l_top, l_bot) = ink_rows(&buf, w, h, &fonts, 9).unwrap(); // 'l'
    assert!(l_top < t_top);
    assert_eq!(l_bot, t_bot);
    // Descender goes below the baseline.
    let (_, g_bot) = ink_rows(&buf, w, h, &fonts, 1).unwrap();
    assert!(g_bot > bottoms[0]);
    check_snapshot("agent_tools", &ascii_snapshot(&buf, w, h));
}

#[test]
fn underscore_sits_below_the_text() {
    let mut fonts = FontStack::embedded(16.0);
    for text in ["agent_tools", "screenshot_2026_10_25"] {
        let (buf, w, h) = render(text, &mut fonts);
        let letter_bottom = text
            .chars()
            .enumerate()
            .filter(|(_, c)| c.is_ascii_alphanumeric() && !"gjpqy".contains(*c))
            .map(|(i, _)| ink_rows(&buf, w, h, &fonts, i).unwrap().1)
            .max()
            .unwrap();
        let letter_top = text
            .chars()
            .enumerate()
            .filter(|(_, c)| c.is_ascii_alphanumeric())
            .map(|(i, _)| ink_rows(&buf, w, h, &fonts, i).unwrap().0)
            .min()
            .unwrap();
        for (i, _) in text.chars().enumerate().filter(|(_, c)| *c == '_') {
            let (top, _) = ink_rows(&buf, w, h, &fonts, i).unwrap();
            assert!(top >= letter_bottom, "{text}: underscore at row {top} is not under the text (bottom {letter_bottom})");
            assert!(top > letter_top);
        }
    }
    let (buf, w, h) = render("screenshot_2026_10_25", &mut fonts);
    check_snapshot("screenshot_2026_10_25", &ascii_snapshot(&buf, w, h));
}

#[test]
fn cells_are_uniform_and_fit_the_font() {
    for size in [12.0, 15.0, 16.0, 22.0] {
        let fonts = FontStack::embedded(size);
        assert!(fonts.cell_w > 0 && fonts.cell_h >= size as i32, "size {size}: {}x{}", fonts.cell_w, fonts.cell_h);
        assert!(fonts.ascent > 0 && fonts.ascent < fonts.cell_h);
        let (cols, rows) = mouser_render::grid_size(&fonts, 800, 600);
        assert!(cols as i32 * fonts.cell_w + 2 * PADDING <= 800);
        assert!(rows as i32 * fonts.cell_h + 2 * PADDING <= 600);
    }
    // Wide and narrow glyphs advance by exactly one cell.
    let mut fonts = FontStack::embedded(16.0);
    let (buf, w, h) = render("iWiW", &mut fonts);
    let left_edge = |i: usize| {
        let x0 = PADDING as usize + i * fonts.cell_w as usize;
        (x0..x0 + fonts.cell_w as usize).find(|&x| (0..h).any(|y| buf[y * w + x] != BG)).unwrap()
    };
    assert_eq!(left_edge(2) - left_edge(0), 2 * fonts.cell_w as usize);
    assert_eq!(left_edge(3) - left_edge(1), 2 * fonts.cell_w as usize);
}

#[test]
fn system_font_stack_has_monospace_cells() {
    let fonts = FontStack::system(None, 16.0);
    assert!(fonts.font_count() >= 1);
    assert!(fonts.cell_w >= 6 && fonts.cell_h >= 16);
}

#[test]
fn sidebar_labels_are_clean_and_buttons_fit() {
    let mut fonts = FontStack::embedded(16.0);
    let mut groups = BTreeMap::new();
    groups.insert(Category::Ssh, vec!["prod".to_string(), "a-very-long-connection-name-that-overflows".to_string()]);
    let (w, h) = (1000usize, 600usize);
    let layout = ui::build_sidebar_layout_metrics(w as i32, h as i32, fonts.cell_w, fonts.cell_h, &groups, None);
    let s = layout.sidebar_rect;
    for cat in &layout.categories {
        for r in [cat.add_rect, cat.header_rect].into_iter().chain(cat.items.iter().flat_map(|i| [i.edit_rect, i.delete_rect])) {
            assert!(r.x >= s.x && r.x + r.w <= s.x + s.w, "{r:?} outside sidebar {s:?}");
        }
        assert!(cat.add_rect.w >= ui::ADD_LABEL.len() as i32 * fonts.cell_w);
    }
    let mut buf = vec![0u32; w * h];
    let lines: Vec<String> = vec![];
    let mut canvas = Canvas::new(&mut buf, w, h);
    render_frame(&mut canvas, &mut fonts, &Frame { lines: &lines, cursor: None, bg: BG, fg: FG, ui: Some(&layout) });
    // No pixel inside the sidebar may show the terminal background (the old
    // renderer blended labels against it, leaving blocks around each glyph).
    for y in s.y as usize..(s.y + s.h) as usize {
        for x in (s.x + 1) as usize..(s.x + s.w) as usize {
            assert_ne!(buf[y * w + x], BG, "terminal background leaked at {x},{y}");
        }
    }
    // Header labels have ink and stay readable (some pixels near FG).
    let hr = layout.categories[0].header_rect;
    let bright =
        (hr.y..hr.y + hr.h).flat_map(|y| (hr.x..hr.x + hr.w).map(move |x| (x, y))).filter(|&(x, y)| buf[y as usize * w + x as usize] == FG).count();
    assert!(bright > 10);
}
