//! The gallery end to end, as a third-party plugin runs: Mouser's adapter
//! starts the real `mouser-gallery` program, which only registers a hosted
//! grid and player. Mouser draws the tiles and thumbnails, the arrows and
//! `v` move and relayout it, Enter opens the player, a click goes
//! fullscreen and Esc comes back; the plugin hears each step as `view.*` /
//! `media.*` events (it shows them in its title).

use mouser_core::modes::Keymap;
use mouser_core::permissions::{Decision, Store};
use mouser_render::modes::stdio::StdioMode;
use mouser_render::modes::{Context, Mode, View};
use mouser_render::ui::{PanelStyle, UiAction};
use std::time::{Duration, Instant};

fn ctx() -> Context {
    Context {
        viewport: mouser_render::UiViewport { width: 1200, height: 900, cell_w: 8, cell_h: 16 },
        area: mouser_render::ui::Rect { x: 0, y: 0, w: 1200, h: 900 },
        reduce_motion: true,
    }
}

fn until(m: &mut StdioMode, what: &str, mut f: impl FnMut(&View) -> bool) -> View {
    let keys = Keymap::new(&[]);
    let end = Instant::now() + Duration::from_secs(15);
    let mut last = View::default();
    while Instant::now() < end {
        last = m.view(&ctx(), &keys);
        if f(&last) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}: {:#?}", last.dock.map(|d| d.items.into_iter().map(|i| (i.text, i.style)).collect::<Vec<_>>()));
}

fn items(v: &View) -> impl Iterator<Item = &mouser_render::ui::PanelItem> {
    v.dock.iter().flat_map(|d| &d.items)
}

fn title(v: &View) -> String {
    items(v).find(|i| i.style == PanelStyle::Title).map(|i| i.text.clone()).unwrap_or_default()
}

fn selected_tile(v: &View) -> Option<i32> {
    items(v).find_map(|i| match (&i.style, &i.action) {
        (PanelStyle::Tile { selected: true }, UiAction::Mode { arg, .. }) => Some(*arg),
        _ => None,
    })
}

#[test]
fn a_third_party_gallery_runs_on_mouser_views_and_player() {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: the only test in this binary; set before anything reads it.
    unsafe { std::env::set_var("MOUSER_HOME", home.path()) };
    let dir = home.path().join("plugins/gallery");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = env!("CARGO_BIN_EXE_mouser-gallery").replace('\\', "\\\\");
    let manifest = include_str!("../plugin.toml").replace("command = [\"./mouser-gallery\"]", &format!("command = [\"{exe}\"]"));
    std::fs::write(dir.join("plugin.toml"), manifest).unwrap();
    let mut store = Store::load();
    store.set("gallery", "run", Decision::Allow);
    store.set("gallery", "files.read", Decision::Allow);
    store.save().unwrap();
    let pics = home.path().join("pics");
    std::fs::create_dir_all(&pics).unwrap();
    for (n, c) in [("a.png", [255, 0, 0, 255]), ("b.png", [0, 255, 0, 255]), ("c.png", [0, 0, 255, 255])] {
        image::RgbaImage::from_pixel(64, 48, image::Rgba(c)).save(pics.join(n)).unwrap();
    }

    let p = mouser_core::plugins::find("gallery").expect("gallery plugin");
    let mut m = StdioMode::new(p, 3).unwrap();
    m.activate();
    m.cwd_changed(&pics);
    // Mouser draws the plugin's list as a list first, then the plugin's title.
    until(&mut m, "the hosted list", |v| title(v).contains("3 files") && items(v).filter(|i| i.text.ends_with(".png")).count() == 3);
    assert!(m.wants_key(b"\x1b[B"), "a focused hosted list takes the arrows");

    // ↓ moves Mouser's selection; the plugin hears view.selected.
    assert!(m.text(b"\x1b[B").is_some());
    until(&mut m, "view.selected in the plugin", |v| title(v).contains("selected b.png"));

    // v: the grid, with the thumbnails Mouser loaded (one image per tile).
    m.text(b"v");
    let v = until(&mut m, "the grid with thumbnails", |v| title(v).contains("layout grid") && v.images.len() == 3);
    assert_eq!(selected_tile(&v), Some(1));
    assert_eq!(items(&v).filter(|i| matches!(i.style, PanelStyle::Tile { .. })).count(), 3);
    m.text(b"\x1b[C");
    let v = until(&mut m, "→ in the grid", |v| title(v).contains("selected c.png"));
    assert_eq!(selected_tile(&v), Some(2));

    // A click on another tile selects it; a second click opens it.
    m.action("__hosted.item:files", 0);
    until(&mut m, "the click's selection", |v| title(v).contains("selected a.png") && selected_tile(v) == Some(0));
    m.action("__hosted.item:files", 0);
    let v = until(&mut m, "the hosted player", |v| {
        title(v).contains("opened a.png")
            && items(v).any(|i| matches!(&i.action, UiAction::Mode { action, .. } if action == "__hosted.media_full"))
            && v.images.len() == 1
    });
    assert!(!m.wants_key(b"\x1b[C"), "no hosted list is focused now");

    // Fullscreen over the dock, then Esc closes it (Mouser takes that Esc).
    let full = items(&v).find_map(|i| match &i.action {
        UiAction::Mode { action, arg } if action == "__hosted.media_full" => Some(*arg),
        _ => None,
    });
    m.action("__hosted.media_full", full.unwrap());
    until(&mut m, "fullscreen", |v| items(v).any(|i| i.text.contains("close esc")) && title(v).contains("fullscreen"));
    assert!(m.wants_key(b" "), "the fullscreen player takes its keys");
    assert!(m.escape());
    until(&mut m, "fullscreen closed", |v| !items(v).any(|i| i.text.contains("close esc")) && title(v).contains("paused"));

    // The plugin's own Esc (Wants.escape) goes back to the folder.
    assert!(m.escape());
    until(&mut m, "back in the folder", |v| items(v).filter(|i| matches!(i.style, PanelStyle::Tile { .. })).count() == 3);
}
