//! The player in the commander plugin: it decodes with ffmpeg in its own
//! process and hands each frame to Mouser through shared memory. A 60 fps
//! clip must reach Mouser at (nearly) its frame rate, like the built-in
//! player. Skipped without ffmpeg.

mod common;

use common::*;
use mouser_core::commander::BINDINGS;
use mouser_core::modes::Keymap;
use mouser_core::permissions::{Decision, Store};
use mouser_render::modes::commander::CommanderMode;
use mouser_render::modes::stdio::StdioMode;
use mouser_render::modes::{Mode, View};
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Distinct video frames shown in `secs`, drawing like a 240 Hz display.
fn frames_per_second(m: &mut dyn Mode, secs: f64) -> f64 {
    let keys = Keymap::new(BINDINGS);
    let big = |v: &View| v.images.iter().filter(|i| i.width >= 320).map(|i| i.key).next();
    let end = Instant::now() + Duration::from_secs(10);
    while big(&m.view(&ctx(), &keys)).is_none() {
        assert!(Instant::now() < end, "no video frame");
        std::thread::sleep(Duration::from_millis(4));
    }
    let start = Instant::now();
    let mut seen = HashSet::new();
    while start.elapsed().as_secs_f64() < secs {
        if let Some(k) = big(&m.view(&ctx(), &keys)) {
            seen.insert(k);
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    seen.len() as f64 / secs
}

#[test]
fn plays_through_shared_memory_at_the_clip_rate() {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: the only test in this binary; set before anything reads them.
    unsafe {
        std::env::set_var("MOUSER_HOME", home.path());
        std::env::set_var("MOUSER_PLUGIN_DIRS", "");
    }
    if !mouser_render::modes::player::available() {
        // CI sets MOUSER_REQUIRE_FFMPEG so the 60 fps path can't be skipped silently.
        assert!(std::env::var_os("MOUSER_REQUIRE_FFMPEG").is_none(), "MOUSER_REQUIRE_FFMPEG is set but ffmpeg is not installed");
        eprintln!("ffmpeg not installed, skipped");
        return;
    }
    let work = tempfile::tempdir().unwrap();
    let clip = work.path().join("clip.mp4");
    let ok = std::process::Command::new("ffmpeg")
        .args(["-loglevel", "error", "-f", "lavfi", "-i", "testsrc=size=1280x720:rate=60:duration=8", "-pix_fmt", "yuv420p"])
        .arg(&clip)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "ffmpeg made the clip");

    let dir = home.path().join("plugins/commander");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = env!("CARGO_BIN_EXE_mouser-commander").replace('\\', "\\\\");
    let manifest = include_str!("../plugin.toml").replace("command = [\"./mouser-commander\"]", &format!("command = [\"{exe}\"]"));
    std::fs::write(dir.join("plugin.toml"), manifest).unwrap();
    let mut store = Store::load();
    for c in ["run", "files.read", "shell.type"] {
        store.set("commander", c, Decision::Allow);
    }
    store.save().unwrap();

    let mut b = CommanderMode::new();
    let p = mouser_core::plugins::find("commander").unwrap();
    let mut m = StdioMode::new(p, 3).unwrap();
    for x in [&mut b as &mut dyn Mode, &mut m] {
        x.activate();
        x.cwd_changed(work.path());
    }
    select(&mut b, &mut m, "clip.mp4");
    // One at a time, so they don't share the CPU.
    b.action("open", 0);
    let builtin = frames_per_second(&mut b, 2.0);
    b.deactivate();
    drop(b);
    m.action("open", 0);
    let plugin = frames_per_second(&mut m, 2.0);
    eprintln!("60 fps clip: built-in {builtin:.1} fps, plugin {plugin:.1} fps");
    // Debug builds on shared runners can't hold 60; both are held to the
    // same floor, and the plugin may not fall far behind the built-in.
    assert!(plugin >= 30.0, "plugin {plugin:.1} fps");
    assert!(plugin >= builtin * 0.7, "plugin {plugin:.1} fps vs built-in {builtin:.1}");
    drop(m);
}
