//! Commander's views drawn on the CPU canvas: list, grid and carousel with
//! their image previews. `MOUSER_TEST_PNG_DIR=dir` saves the frames.

use mouser_render::modes::{Context, Effect, Host, ModeImage};
use mouser_render::ui::{self, Rect};
use mouser_render::{Canvas, FontStack, Frame, ImageDraw, PADDING, UiViewport, render_frame};

const COLORS: [[u8; 3]; 4] = [[220, 60, 60], [60, 160, 90], [60, 90, 220], [230, 180, 40]];

fn demo() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    for (i, c) in COLORS.iter().enumerate() {
        image::RgbImage::from_pixel(320, 200, image::Rgb(*c)).save(t.path().join(format!("shot{i}.png"))).unwrap();
    }
    std::fs::write(t.path().join("README.md"), "# Demo\n\nCommander lists this folder.\n").unwrap();
    std::fs::create_dir(t.path().join("photos")).unwrap();
    std::fs::write(t.path().join("clip.mp4"), b"").unwrap();
    t
}

fn draw(m: &ModeImage, top: i32) -> ImageDraw {
    ImageDraw {
        key: m.key,
        rgba: m.rgba.clone(),
        img_w: m.width,
        img_h: m.height,
        src: m.src,
        col: 0,
        row: 0,
        x_off: m.rect.x - PADDING,
        y_off: m.rect.y - top,
        width: m.rect.w as u32,
        height: m.rect.h as u32,
        z: 1,
    }
}

/// Render the host's view (waiting for `n` previews); returns the pixels.
fn frame(h: &mut Host, n: usize, name: &str) -> (Vec<u32>, Vec<ModeImage>) {
    let (w, hh) = (1100usize, 760usize);
    let vp = UiViewport { width: w as i32, height: hh as i32, cell_w: 9, cell_h: 18 };
    let ctx = Context { viewport: vp, area: Rect { x: 0, y: PADDING, w: w as i32, h: hh as i32 - PADDING } };
    let mut v = h.view(&ctx);
    for _ in 0..300 {
        if v.images.len() >= n {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        v = h.view(&ctx);
    }
    let dock = v.dock.clone().unwrap();
    let inset = dock.rect.h + 8;
    let mut layout = ui::without_sidebar(w as i32, hh as i32);
    layout.dock = Some(ui::Dock { panel: dock, inset, fullscreen: false });
    let draws: Vec<ImageDraw> = v.images.iter().map(|m| draw(m, PADDING + inset)).collect();
    let lines = vec!["$ ls".to_string()];
    let mut fonts = FontStack::embedded(15.0);
    let mut buf = vec![0u32; w * hh];
    let fr = Frame {
        lines: &lines,
        cursor: Some((5, 0)),
        bg: 0x0B1E2A,
        fg: 0xD8E6EE,
        ui: Some(&layout),
        status: None,
        accent: Some(0x2FD8E5),
        cursor_color: None,
        images: &draws,
        styles: &[],
        cursor_shape: Default::default(),
        tabs: None,
    };
    render_frame(&mut Canvas::new(&mut buf, w, hh), &mut fonts, &fr);
    if let Some(d) = std::env::var_os("MOUSER_TEST_PNG_DIR") {
        let rgba: Vec<u8> = buf.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]).collect();
        image::save_buffer(std::path::Path::new(&d).join(format!("commander-{name}.png")), &rgba, w as u32, hh as u32, image::ColorType::Rgba8)
            .unwrap();
    }
    (buf, v.images)
}

fn has_color(buf: &[u32], r: Rect, c: [u8; 3]) -> bool {
    let want = (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32;
    (r.y..r.y + r.h).any(|y| (r.x..r.x + r.w).any(|x| buf[y as usize * 1100 + x as usize] & 0xFFFFFF == want))
}

#[test]
fn list_grid_and_carousel_draw_with_previews() {
    let t = demo();
    let mut h = Host::builtin();
    h.set_cwd(Some(t.path().to_path_buf()));
    let (_, imgs) = frame(&mut h, 0, "list");
    assert!(imgs.is_empty(), "the list loads no previews");
    // Defaults show . and ..; hide them for this test.
    h.action("hidden", 0);
    h.key(b"\x1bt").unwrap();
    let (buf, imgs) = frame(&mut h, 4, "grid");
    assert_eq!(imgs.len(), 4, "every visible image tile");
    for (m, c) in imgs.iter().zip(COLORS) {
        assert!(has_color(&buf, m.rect, c), "preview drawn at {:?}", m.rect);
    }
    // Carousel on shot1: shot0 and shot2 are the partial neighbours.
    h.key(b"\x1bt").unwrap();
    // clip.mp4, photos, README.md, shot0, shot1: four steps right.
    for _ in 0..4 {
        h.key(b"\x1b[1;3C").unwrap();
    }
    let (buf, imgs) = frame(&mut h, 3, "carousel");
    assert_eq!(imgs.len(), 3);
    let centre = imgs.iter().max_by_key(|m| m.rect.w).unwrap();
    assert!(has_color(&buf, centre.rect, COLORS[1]), "focused item in the middle");
    assert!(imgs.iter().filter(|m| m.src.2 < m.width).count() == 2, "two partial neighbours");
}

/// Mouse input as the window routes it (`ui::dock_press`/`dock_wheel`, the
/// same calls the event loop makes): click a row to select it, click a
/// selected folder to open it, click the back button, wheel over the view.
#[test]
fn mouse_clicks_and_wheel_drive_commander() {
    let t = demo();
    let mut h = Host::builtin();
    h.set_cwd(Some(t.path().to_path_buf()));
    let vp = UiViewport { width: 1100, height: 760, cell_w: 9, cell_h: 18 };
    let ctx = Context { viewport: vp, area: Rect { x: 0, y: PADDING, w: 1100, h: 760 - PADDING } };
    let layout = |h: &mut Host| {
        let mut l = ui::without_sidebar(1100, 760);
        let dock = h.view(&ctx).dock.unwrap();
        l.dock = Some(ui::Dock { inset: dock.rect.h + 8, panel: dock, fullscreen: false });
        l
    };
    let find = |l: &ui::UiLayout, s: &str| {
        l.dock.as_ref().unwrap().panel.items.iter().find(|i| i.text.contains(s)).cloned().unwrap_or_else(|| panic!("no {s}"))
    };
    let click = |h: &mut Host, s: &str| {
        let l = layout(h);
        let r = find(&l, s).rect;
        let (x, y) = (r.x + r.w / 2, r.y + r.h / 2);
        let acts = ui::dock_press(&l, x, y).expect("over the view");
        assert!(matches!(&acts[0], ui::UiAction::Mode { action, .. } if action == "pointer_down"));
        let mut last = None;
        for a in acts {
            if let ui::UiAction::Mode { action, arg } = a {
                last = Some(h.action(&action, arg));
            }
        }
        h.action("pointer_up", mouser_render::modes::commander::pack_xy(x, y));
        last.unwrap()
    };
    let selected = |h: &mut Host| {
        let l = layout(h);
        l.dock.unwrap().panel.items.into_iter().find(|i| i.style == ui::PanelStyle::Row { selected: true }).map(|i| i.text).unwrap_or_default()
    };
    assert!(selected(&mut h).contains("clip.mp4"));
    click(&mut h, "README.md");
    assert!(selected(&mut h).contains("README.md"), "a click selects the row");
    click(&mut h, "photos/");
    assert!(selected(&mut h).contains("photos/"));
    // A quick second click opens the folder: commander browses it (the shell stays).
    let e = click(&mut h, "photos/");
    assert!(matches!(&e, Effect::None), "{e:?}");
    let crumbs =
        layout(&mut h).dock.unwrap().panel.items.into_iter().filter(|i| i.style == ui::PanelStyle::Title).map(|i| i.text).collect::<String>();
    assert!(crumbs.ends_with("photos"), "now in photos: {crumbs}");
    let e = click(&mut h, "‹ back");
    assert!(matches!(&e, Effect::None), "the back button browses back: {e:?}");
    // The wheel: over the view it scrolls the mode; outside the view it doesn't.
    let l = layout(&mut h);
    let row = find(&l, "shot0").rect;
    assert_eq!(ui::dock_wheel(&l, row.x + 5, row.y + 5, -3, false), Some(vec![ui::UiAction::Mode { action: "scroll".into(), arg: 1 }]));
    let z = ui::dock_wheel(&l, row.x + 5, row.y + 5, 3, true).unwrap();
    assert!(matches!(&z[1], ui::UiAction::Mode { action, arg: 1 } if action == "zoom_wheel"));
    assert_eq!(ui::dock_wheel(&l, 5, 755, -3, false), None, "below the view");
    assert_eq!(ui::dock_press(&l, 5, 755), None);
    for a in ui::dock_wheel(&l, row.x + 5, row.y + 5, -3, false).unwrap() {
        if let ui::UiAction::Mode { action, arg } = a {
            h.action(&action, arg);
        }
    }
}

#[test]
fn restore_picks_the_remembered_mode_or_none() {
    let mut h = Host::builtin();
    assert_eq!(h.current(), Some("commander"));
    h.restore(None);
    assert_eq!(h.current(), None, "no mode");
    h.restore(Some("commander"));
    assert_eq!(h.current(), Some("commander"));
    h.restore(Some("gone-plugin"));
    assert_eq!(h.current(), None, "a missing or disabled mode falls back to no mode");
}

/// Wait for the carousel's slide to finish, so a later view draws the same
/// layout as the one the test reads.
fn settle(h: &mut Host) {
    let vp = UiViewport { width: 1100, height: 760, cell_w: 9, cell_h: 18 };
    let ctx = Context { viewport: vp, area: Rect { x: 0, y: PADDING, w: 1100, h: 760 - PADDING } };
    let rects = |h: &mut Host| h.view(&ctx).dock.map(|d| d.items.iter().map(|i| i.rect).collect::<Vec<_>>());
    let mut last = rects(h);
    for _ in 0..200 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        let now = rects(h);
        if now == last {
            return;
        }
        last = now;
    }
}

/// Carousel names float over the preview (drawn after the image), and a
/// folder shows a folder icon in the middle of its box.
#[test]
fn carousel_floating_names_and_folder_icon() {
    let t = demo();
    let mut h = Host::builtin();
    h.set_cwd(Some(t.path().to_path_buf()));
    h.action("hidden", 0);
    h.key(b"\x1bt").unwrap();
    h.key(b"\x1bt").unwrap();
    // clip.mp4, photos: one step right is the folder.
    h.key(b"\x1b[1;3C").unwrap();
    settle(&mut h);
    let (buf, _) = frame(&mut h, 0, "carousel-folder");
    let vp = UiViewport { width: 1100, height: 760, cell_w: 9, cell_h: 18 };
    let ctx = Context { viewport: vp, area: Rect { x: 0, y: PADDING, w: 1100, h: 760 - PADDING } };
    let items = h.view(&ctx).dock.unwrap().items;
    let icon = items.iter().find(|i| i.style == ui::PanelStyle::FolderIcon).expect("folder icon").rect;
    let label = items.iter().find(|i| i.style == ui::PanelStyle::FloatLabel && i.text == "photos").expect("floating name").rect;
    assert!(icon.y + icon.h <= label.y, "name under the icon");
    let px = |x: i32, y: i32| buf[y as usize * 1100 + x as usize];
    // The icon is drawn: its middle differs from the tile around it.
    assert_ne!(px(icon.x + icon.w / 2, icon.y + icon.h * 3 / 4), px(icon.x - 4, icon.y + icon.h / 2));
    // shot0, shot1: the label sits on top of the image, see-through.
    for _ in 0..3 {
        h.key(b"\x1b[1;3C").unwrap();
    }
    settle(&mut h);
    let (buf, imgs) = frame(&mut h, 3, "carousel-file");
    let centre = imgs.iter().max_by_key(|m| m.rect.w).unwrap().rect;
    let items = h.view(&ctx).dock.unwrap().items;
    let label = items.iter().find(|i| i.style == ui::PanelStyle::FloatLabel && i.text == "shot1.png").expect("floating name").rect;
    let (x, y) = (label.x + 2, label.y + label.h / 2);
    assert!(x >= centre.x && y < centre.y + centre.h, "label over the preview {label:?} {centre:?}");
    let p = buf[y as usize * 1100 + x as usize];
    let g = COLORS[1];
    assert_ne!(p & 0xFFFFFF, (g[0] as u32) << 16 | (g[1] as u32) << 8 | g[2] as u32, "label drawn after the image");
    assert!((p >> 8) & 0xFF > (p >> 16) & 0xFF, "the green shows through the label");
}

const RICH_MD: &str = "# Mouser\n\nA **GPU** terminal with *modes* and `commander`.\n\n## Features\n\n- grid and carousel\n- [docs](https://mouser.weirdbrains.com)\n\n> quoted note\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\n| key | does |\n|-----|------|\n| t | view |\n\n---\n";

/// Markdown files preview rendered (headings, code blocks, quotes), not as
/// raw text: in the grid tile, the carousel box and the file view.
#[test]
fn markdown_previews_render() {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("README.md"), RICH_MD).unwrap();
    std::fs::write(t.path().join("zz.txt"), "# not markdown\n").unwrap();
    let mut h = Host::builtin();
    h.set_cwd(Some(t.path().to_path_buf()));
    h.action("hidden", 0);
    let vp = UiViewport { width: 1100, height: 760, cell_w: 9, cell_h: 18 };
    let ctx = Context { viewport: vp, area: Rect { x: 0, y: PADDING, w: 1100, h: 760 - PADDING } };
    let md_items = |h: &mut Host| -> Vec<ui::PanelItem> {
        h.view(&ctx)
            .dock
            .unwrap()
            .items
            .into_iter()
            .filter(|i| matches!(i.style, ui::PanelStyle::Md { .. } | ui::PanelStyle::MdCode { .. } | ui::PanelStyle::MdToken(_)))
            .collect()
    };
    for (key, name) in [(&b"\x1bt"[..], "markdown-grid"), (&b"\x1bt"[..], "markdown-carousel")] {
        h.key(key).unwrap();
        settle(&mut h);
        frame(&mut h, 0, name);
        let items = md_items(&mut h);
        assert!(items.iter().any(|i| i.text == "Mouser" && i.style == ui::PanelStyle::Md { flags: 1, heading: 1 }), "{name}: heading");
        assert!(!items.iter().any(|i| i.text.contains('#')), "{name}: no raw markup");
        assert!(!items.iter().any(|i| i.text == "not markdown"), "{name}: .txt stays raw");
    }
    // The file view: every block, the code block on its background.
    h.action("open", 0);
    settle(&mut h);
    let (buf, _) = frame(&mut h, 0, "markdown-view");
    let items = md_items(&mut h);
    let code = items.iter().find(|i| matches!(i.style, ui::PanelStyle::MdCode { top: true, .. })).expect("code block").rect;
    assert!(items.iter().any(|i| i.text == "println!(\"hi\");" || i.text.contains("println")), "code text");
    // The rust block is highlighted: `fn` a keyword, the string a string.
    use mouser_core::highlight::{KEYWORD, STRING};
    assert!(items.iter().any(|i| i.text == "fn" && i.style == ui::PanelStyle::MdToken(KEYWORD)), "highlighted keyword");
    assert!(items.iter().any(|i| i.text == "\"hi\"" && i.style == ui::PanelStyle::MdToken(STRING)), "highlighted string");
    assert!(items.iter().any(|i| i.text == "docs" && matches!(i.style, ui::PanelStyle::Md { flags, .. } if flags & 8 != 0)), "link");
    assert!(items.iter().any(|i| i.text.contains("│")), "table");
    let px = |x: i32, y: i32| buf[y as usize * 1100 + x as usize] & 0xFFFFFF;
    assert_ne!(px(code.x + code.w - 4, code.y + code.h / 2), px(code.x + code.w - 4, code.y - 30), "code background drawn");
}
