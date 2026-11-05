//! What the through-Mouser tests share: the window, and drawing the plugin
//! through Mouser's adapter until its view settles.

#![allow(dead_code)]

use mouser_core::commander::BINDINGS;
use mouser_core::modes::Keymap;
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

/// Draw the plugin until its view settles (two frames alike; thumbnails
/// decode on threads) with a dock.
pub fn settle(p: &mut StdioMode, what: &str) -> View {
    settle_when(p, what, |_| true)
}

/// Draw the plugin until its view settles and `ready` holds for it.
pub fn settle_when(p: &mut StdioMode, what: &str, ready: impl Fn(&View) -> bool) -> View {
    let keys = Keymap::new(BINDINGS);
    let end = Instant::now() + Duration::from_secs(20);
    let mut last = String::new();
    loop {
        let v = p.view(&ctx(), &keys);
        let s = shape(&v);
        if s == last && v.dock.is_some() && ready(&v) {
            return v;
        }
        if Instant::now() > end {
            let _ = std::fs::write(std::env::temp_dir().join("commander-plugin.txt"), &s);
            panic!("{what}: the plugin's view never settled (commander-plugin.txt in the temp dir)\n{s}");
        }
        last = s;
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Whether any dock item's text contains `t`.
pub fn shows(v: &View, t: &str) -> bool {
    v.dock.iter().flat_map(|d| &d.items).any(|i| i.text.contains(t))
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

/// Move down from the top to the row named `name`.
pub fn select(m: &mut StdioMode, name: &str) {
    m.action("first", 0);
    for _ in 0..20 {
        let v = settle(m, name);
        let sel = v
            .dock
            .iter()
            .flat_map(|d| &d.items)
            .find(|i| matches!(i.style, mouser_render::ui::PanelStyle::Row { selected: true }))
            .map(|i| i.text.clone());
        if sel.as_deref().is_some_and(|t| t.split_whitespace().any(|w| w.trim_end_matches('/') == name)) {
            return;
        }
        m.action("down", 0);
    }
    panic!("{name} not in the list");
}

/// ffmpeg is on the PATH (the player needs it).
pub fn ffmpeg() -> bool {
    std::process::Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
}

pub fn png(p: &Path, w: u32, h: u32) {
    image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x * 7) as u8, (y * 5) as u8, 90, 255])).save(p).unwrap();
}
