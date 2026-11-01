//! Isolated custom engines: the example engine runs in `mouser __engine-host`;
//! its pixels match the CPU reference, and a crash inside it (abort on the
//! n-th frame) is reported as EngineCrashed while this process and a shell
//! running in a PTY carry on. Unix only (the host is Unix only).
#![cfg(unix)]

use mouser_core::engines;
use mouser_render::engine_host::{EngineCrashed, HostClient};
use mouser_render::{Canvas, DisplayList, FontStack, Frame, ImageDraw, render_frame};
use serial_test::serial;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn example_lib() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let target = root.join("target/engine-tests");
    let st = std::process::Command::new(env!("CARGO"))
        .args(["build", "-q", "-p", "mouser-engine-example", "--target-dir"])
        .arg(&target)
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(st.success());
    target.join("debug").join(format!("{}mouser_engine_flat{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_SUFFIX))
}

fn spawn(w: u32, h: u32) -> anyhow::Result<HostClient> {
    HostClient::spawn(Path::new(env!("CARGO_BIN_EXE_mouser-app")), &["__engine-host".into()], &example_lib(), "flat", w, h)
}

fn scene(fonts: &mut FontStack, w: usize, h: usize, text: &str) -> DisplayList {
    let lines = vec![text.to_string(), "second ▓ line ✓".to_string()];
    let images = vec![ImageDraw {
        key: 9,
        rgba: Arc::new([255u8, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 255, 255, 255, 255, 255].to_vec()),
        img_w: 2,
        img_h: 2,
        src: (0, 0, 2, 2),
        col: 3,
        row: 2,
        x_off: 0,
        y_off: 0,
        width: 30,
        height: 20,
        z: 1,
    }];
    let frame = Frame {
        lines: &lines,
        cursor: Some((2, 0)),
        bg: 0x101820,
        fg: 0xE0E0E0,
        ui: None,
        status: None,
        accent: None,
        cursor_color: None,
        images: &images,
        styles: &[],
        cursor_shape: Default::default(),
        tabs: None,
    };
    let mut list = DisplayList::new(w, h);
    render_frame(&mut list, fonts, &frame);
    list
}

fn close(a: &[u32], b: &[u32]) -> usize {
    a.iter().zip(b).filter(|(x, y)| (0..3).any(|s| (((**x >> (s * 8)) & 0xff) as i32 - ((**y >> (s * 8)) & 0xff) as i32).abs() > 1)).count()
}

#[test]
#[serial]
fn hosted_engine_matches_cpu_reference_and_resizes() {
    unsafe { std::env::remove_var("MOUSER_FLAT_CRASH_AT") };
    let mut fonts = FontStack::embedded(15.0);
    let mut host = spawn(300, 120).unwrap();
    for (w, h, t) in [(300usize, 120usize, "hello host"), (220, 90, "smaller now"), (320, 140, "bigger again")] {
        let list = scene(&mut fonts, w, h, t);
        let mut reference = vec![0u32; w * h];
        list.replay(&mut Canvas::new(&mut reference, w, h), &mut fonts);
        let mut out = vec![0u32; w * h];
        assert!(host.draw_into(&list, &mut fonts, &mut out).unwrap());
        assert_eq!(close(&reference, &out), 0, "{w}x{h}");
    }
}

#[test]
#[serial]
fn start_failure_is_an_error_not_a_crash() {
    unsafe { std::env::set_var("MOUSER_FLAT_FAIL", "create") };
    let e = spawn(10, 10).err().unwrap();
    unsafe { std::env::remove_var("MOUSER_FLAT_FAIL") };
    assert!(e.downcast_ref::<EngineCrashed>().is_none(), "{e:#}");
    assert!(format!("{e:#}").contains("MOUSER_FLAT_FAIL"), "{e:#}");
    let e = HostClient::spawn(Path::new(env!("CARGO_BIN_EXE_mouser-app")), &["__engine-host".into()], &example_lib(), "other", 10, 10).err().unwrap();
    assert!(format!("{e:#}").contains("manifest says"), "{e:#}");
}

#[test]
#[serial]
fn crash_in_host_leaves_shell_running_and_marks_skipped() {
    let t = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("MOUSER_HOME", t.path().join("m"));
        std::env::set_var("HOME", t.path());
    }
    let lib = example_lib();
    let m = engines::Manifest {
        name: "flat".into(),
        version: "0.1.0".into(),
        abi: 1,
        library: lib.file_name().unwrap().to_string_lossy().into(),
        description: String::new(),
    };
    engines::install(&m, &lib).unwrap();
    // A shell in a PTY, owned by this process (like a Mouser tab).
    let mut pty = mouser_pty::spawn_argv(&["/bin/sh".into()], 80, 24, 0, 0).unwrap();
    let pid = pty.child.process_id().unwrap();

    unsafe { std::env::set_var("MOUSER_FLAT_CRASH_AT", "3") };
    let mut host = spawn(200, 80).unwrap();
    unsafe { std::env::remove_var("MOUSER_FLAT_CRASH_AT") };
    let mut fonts = FontStack::embedded(15.0);
    let list = scene(&mut fonts, 200, 80, "crash test");
    let mut out = vec![0u32; 200 * 80];
    assert!(host.draw_into(&list, &mut fonts, &mut out).unwrap());
    assert!(host.draw_into(&list, &mut fonts, &mut out).unwrap());
    let e = host.draw_into(&list, &mut fonts, &mut out).unwrap_err();
    let crash = e.downcast_ref::<EngineCrashed>().expect("crash reported as EngineCrashed");
    assert!(crash.0.contains("flat") && crash.0.contains("signal 6"), "{crash}");
    // What the window does with it:
    engines::mark_skipped("flat", &crash.0);
    assert!(matches!(engines::state("flat"), engines::State::Skipped { reason: Some(r), .. } if r.contains("signal 6")));
    assert!(mouser_render::engine::skip_check("flat").unwrap_err().contains("mouser engine unskip flat"));

    // The shell is the same process and still answers.
    assert_eq!(pty.child.process_id(), Some(pid));
    assert!(pty.child.try_wait().unwrap().is_none());
    pty.writer.write_all(b"echo still-$((40+2))\n").unwrap();
    pty.writer.flush().unwrap();
    let mut got = String::new();
    let mut buf = [0u8; 512];
    let deadline = Instant::now() + Duration::from_secs(5);
    while !got.contains("still-42") && Instant::now() < deadline {
        let n = pty.reader.read(&mut buf).unwrap();
        got.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    assert!(got.contains("still-42"), "{got}");
    let _ = pty.child.kill();

    // Restorable: unskip clears it and a new host works again.
    assert!(engines::unskip("flat").unwrap());
    assert!(mouser_render::engine::skip_check("flat").is_ok());
    let mut host = spawn(200, 80).unwrap();
    assert!(host.draw_into(&list, &mut fonts, &mut out).unwrap());
}
