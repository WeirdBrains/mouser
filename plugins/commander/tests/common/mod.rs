//! What both through-Mouser tests share: the window, and drawing the
//! built-in mode and the plugin side by side until they agree.

#![allow(dead_code)]

use mouser_core::commander::BINDINGS;
use mouser_core::modes::Keymap;
use mouser_render::modes::commander::CommanderMode;
use mouser_render::modes::stdio::StdioMode;
use mouser_render::modes::{Context, Effect, Mode, View};
use std::path::Path;
use std::time::{Duration, Instant};

pub fn ctx() -> Context {
    Context {
        viewport: mouser_render::UiViewport { width: 1200, height: 900, cell_w: 8, cell_h: 16 },
        area: mouser_render::ui::Rect { x: 0, y: 40, w: 1000, h: 840 },
    }
}

/// What must match: every item (rect, text, style, action) and every
/// image (size, source, place and pixels; keys are handed out in decode
/// order, so only their being distinct counts).
pub fn shape(v: &View) -> String {
    let mut keys: Vec<u64> = v.images.iter().map(|i| i.key).collect();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), v.images.len(), "one key per image");
    let imgs: Vec<_> =
        v.images.iter().map(|i| (i.width, i.height, i.src, i.rect, i.rgba.len(), i.rgba.iter().map(|&b| b as u64).sum::<u64>())).collect();
    format!("{:#?}\n{:#?}\n{imgs:?}\n{}", v.dock, v.panel, v.fullscreen)
}

/// Draw both until they agree (thumbnails decode on threads in both).
pub fn same(b: &mut CommanderMode, p: &mut StdioMode, what: &str) -> View {
    same_when(b, p, what, |_| true)
}

/// Draw both until they agree and `ready` holds for the view.
pub fn same_when(b: &mut CommanderMode, p: &mut StdioMode, what: &str, ready: impl Fn(&View) -> bool) -> View {
    let keys = Keymap::new(BINDINGS);
    let end = Instant::now() + Duration::from_secs(20);
    let (mut vb, mut vp);
    loop {
        vb = b.view(&ctx(), &keys);
        vp = p.view(&ctx(), &keys);
        if shape(&vb) == shape(&vp) && vp.dock.is_some() && ready(&vp) {
            return vp;
        }
        if Instant::now() > end {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let t = std::env::temp_dir();
    let _ = std::fs::write(t.join("commander-builtin.txt"), shape(&vb));
    let _ = std::fs::write(t.join("commander-plugin.txt"), shape(&vp));
    panic!(
        "{what}: the plugin draws differently (both in the temp dir: commander-builtin.txt, commander-plugin.txt)\n--- built-in\n{}\n--- plugin\n{}",
        shape(&vb),
        shape(&vp)
    );
}

/// The plugin's effects (they come back after a round trip).
pub fn effects(p: &mut StdioMode, want: usize) -> Vec<Effect> {
    let end = Instant::now() + Duration::from_secs(10);
    let mut got = Vec::new();
    while got.len() < want && Instant::now() < end {
        let _ = p.view(&ctx(), &Keymap::new(BINDINGS));
        got.extend(p.take_effects().into_iter().filter(|e| !matches!(e, Effect::Say(_))));
        std::thread::sleep(Duration::from_millis(10));
    }
    got
}

/// Move both down from the top to the row named `name`.
pub fn select(b: &mut CommanderMode, m: &mut StdioMode, name: &str) {
    for x in [&mut *b as &mut dyn Mode, &mut *m] {
        x.action("first", 0);
    }
    for _ in 0..20 {
        let v = same(b, m, name);
        let sel = v
            .dock
            .iter()
            .flat_map(|d| &d.items)
            .find(|i| matches!(i.style, mouser_render::ui::PanelStyle::Row { selected: true }))
            .map(|i| i.text.clone());
        if sel.as_deref().is_some_and(|t| t.split_whitespace().any(|w| w.trim_end_matches('/') == name)) {
            return;
        }
        b.action("down", 0);
        m.action("down", 0);
    }
    panic!("{name} not in the list");
}

pub fn png(p: &Path, w: u32, h: u32) {
    image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x * 7) as u8, (y * 5) as u8, 90, 255])).save(p).unwrap();
}
