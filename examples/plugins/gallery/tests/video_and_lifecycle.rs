//! The host player on a real clip (a tiny ffmpeg-made video with a silent
//! sound track): Enter opens it, Mouser plays it (frames as images,
//! `media.state` playing, `media.ended`); switching the mode away pauses
//! it. Then the host registry's lifecycle and permissions: taking back
//! `files.read` stops every read on the next frame, and a restarted plugin
//! registers its views again. Skipped without ffmpeg / ffprobe.

use mouser_core::modes::Keymap;
use mouser_core::permissions::{Decision, Store};
use mouser_render::modes::stdio::StdioMode;
use mouser_render::modes::{Context, Mode, View};
use mouser_render::ui::{PanelStyle, UiAction};
use std::process::Command;
use std::time::{Duration, Instant};

fn ctx() -> Context {
    Context {
        viewport: mouser_render::UiViewport { width: 1200, height: 900, cell_w: 8, cell_h: 16 },
        area: mouser_render::ui::Rect { x: 0, y: 0, w: 1200, h: 900 },
        reduce_motion: true,
    }
}

fn until(m: &mut StdioMode, what: &str, mut f: impl FnMut(&View) -> bool) -> View {
    let end = Instant::now() + Duration::from_secs(20);
    let mut last = View::default();
    while Instant::now() < end {
        last = m.view(&ctx(), &Keymap::new(&[]));
        if f(&last) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}: {:?}", last.dock.map(|d| d.items.into_iter().map(|i| i.text).collect::<Vec<_>>()));
}

fn items(v: &View) -> impl Iterator<Item = &mouser_render::ui::PanelItem> {
    v.dock.iter().flat_map(|d| &d.items)
}

fn title(v: &View) -> String {
    items(v).find(|i| i.style == PanelStyle::Title).map(|i| i.text.clone()).unwrap_or_default()
}

fn tiles(v: &View) -> usize {
    items(v).filter(|i| matches!(i.style, PanelStyle::Tile { .. } | PanelStyle::Row { .. })).count()
}

/// A frame of the 96 × 64 clip, at whatever width the player decodes.
fn is_clip(i: &mouser_render::modes::ModeImage) -> bool {
    i.width >= 96 && (i.width as i64 * 64 - i.height as i64 * 96).abs() <= 2 * 96
}

fn grant(cap: &str, d: Decision) {
    let mut s = Store::load();
    s.set("gallery", cap, d);
    s.save().unwrap();
}

#[test]
fn host_player_plays_a_real_clip_and_the_registry_follows_permissions_and_restarts() {
    if !mouser_media::player::available() {
        eprintln!("skipped: no ffmpeg / ffprobe on PATH");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    // SAFETY: the only test in this binary; set before anything reads it.
    unsafe { std::env::set_var("MOUSER_HOME", home.path()) };
    let dir = home.path().join("plugins/gallery");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = env!("CARGO_BIN_EXE_mouser-gallery").replace('\\', "\\\\");
    let manifest = include_str!("../plugin.toml").replace("command = [\"./mouser-gallery\"]", &format!("command = [\"{exe}\"]"));
    std::fs::write(dir.join("plugin.toml"), manifest).unwrap();
    grant("run", Decision::Allow);
    grant("files.read", Decision::Allow);
    let pics = home.path().join("pics");
    std::fs::create_dir_all(&pics).unwrap();
    image::RgbaImage::from_pixel(64, 48, image::Rgba([200, 0, 0, 255])).save(pics.join("b.png")).unwrap();
    let clip = pics.join("a.mp4");
    let st = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=96x64:rate=10:duration=1.5"])
        .args(["-f", "lavfi", "-i", "anullsrc=r=22050:cl=mono", "-t", "1.5", "-c:v", "mpeg4", "-c:a", "aac", "-shortest"])
        .arg(&clip)
        .status()
        .expect("ffmpeg");
    assert!(st.success() && clip.is_file());

    let p = mouser_core::plugins::find("gallery").expect("gallery plugin");
    let mut m = StdioMode::new(p, 3).unwrap();
    m.activate();
    m.cwd_changed(&pics);
    until(&mut m, "the list", |v| title(v).contains("2 files") && tiles(v) == 2);

    // a.mp4 is first: Enter opens it and Mouser plays it.
    m.text(b"\r");
    let v = until(&mut m, "the clip playing", |v| title(v).contains("playing") && v.images.iter().any(is_clip));
    let first = v.images.iter().find(|i| is_clip(i)).unwrap().key;
    let v = until(&mut m, "a later frame", |v| v.images.iter().any(|i| is_clip(i) && i.key != first));
    assert!(items(&v).any(|i| matches!(i.style, PanelStyle::Progress { .. })));

    // Switching the mode away pauses it; Mouser tells the plugin.
    m.deactivate();
    until(&mut m, "paused when hidden", |v| title(v).contains("paused"));
    m.activate();
    let play =
        until(&mut m, "the play button", |v| items(v).any(|i| matches!(&i.action, UiAction::Mode { action, .. } if action == "__hosted.media_play")));
    let arg = items(&play)
        .find_map(|i| match &i.action {
            UiAction::Mode { action, arg } if action == "__hosted.media_play" => Some(*arg),
            _ => None,
        })
        .unwrap();
    m.action("__hosted.media_play", arg);
    until(&mut m, "playing again, to the end", |v| title(v).contains("ended"));

    // Taking back files.read: on the next frame nothing is read.
    grant("files.read", Decision::Never);
    let v = until(&mut m, "the player without files.read", |v| items(v).any(|i| i.text == "a.mp4 (needs files.read)"));
    assert!(v.images.is_empty(), "no frames or thumbnails without files.read");
    grant("files.read", Decision::Allow);

    // A restarted plugin registers its views again (fresh state: the list).
    m.action("__mouser_restart", 0);
    until(&mut m, "the views after a restart", |v| title(v).contains("(no folder yet)"));
    m.cwd_changed(&pics);
    until(&mut m, "the list after a restart", |v| tiles(v) == 2 && !items(v).any(|i| i.text.contains("needs files.read")));
}
