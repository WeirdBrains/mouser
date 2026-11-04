//! The go/no-go gate from the mode-split plan: a real child process, the
//! handshake, and 1080p RGBA frames at 60 fps through shared memory (and
//! inline). `cargo test -p mouser-mode-api --release -- --ignored` runs the
//! full 10 s benchmark with no dropped frames allowed.
use mouser_mode_api::codec::{self, Wire};
use mouser_mode_api::protocol::{ApiRange, HostMsg, ModeMsg, Pixels, Welcome, negotiate};
use mouser_mode_api::shm::Ring;
use mouser_mode_api::{Context, Effect, Rect, Viewport};
use std::io::{BufReader, BufWriter};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

const FRAME: usize = 1920 * 1080 * 4;

struct Plugin {
    child: Child,
    tx: BufWriter<ChildStdin>,
    rx: BufReader<ChildStdout>,
    buf: Vec<u8>,
}

fn spawn(args: &[&str]) -> (Plugin, ApiRange) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mouser-mode-echo")).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let tx = BufWriter::new(child.stdin.take().unwrap());
    let rx = BufReader::with_capacity(1 << 20, child.stdout.take().unwrap());
    let mut p = Plugin { child, tx, rx, buf: Vec::new() };
    let Some(ModeMsg::Hello(h)) = codec::read(&mut p.rx, Wire::MsgPack, &mut p.buf).unwrap() else { panic!("no hello") };
    (p, h.api)
}

fn ctx() -> Context {
    Context { viewport: Viewport { width: 1920, height: 1080, cell_w: 10, cell_h: 20 }, area: Rect { x: 0, y: 0, w: 1920, h: 1080 } }
}

fn accept(p: &mut Plugin, api: ApiRange, ring: Option<&Ring>) {
    let api = negotiate(ApiRange::CURRENT, api).unwrap();
    codec::write(&mut p.tx, Wire::MsgPack, &Welcome::Accepted { api, ctx: ctx(), granted: vec![], shm: ring.map(Ring::spec) }).unwrap();
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p) as usize]
}

/// Frames, dropped, p50/p99 round trip ms, p99 host overhead ms.
fn run(shm: bool, secs: u64) -> (u64, u64, f64, f64, f64) {
    let (mut p, api) = spawn(&[]);
    let ring = shm.then(|| Ring::create("bench", 2, FRAME).unwrap());
    accept(&mut p, api, ring.as_ref());
    let mut tex = vec![0u8; FRAME];
    let period = Duration::from_secs_f64(1.0 / 60.0);
    let n = secs * 60;
    let (mut rtt, mut host) = (Vec::new(), Vec::new());
    let mut dropped = 0;
    let start = Instant::now();
    for seq in 0..n {
        let deadline = start + period * (seq as u32 + 1);
        let t = Instant::now();
        codec::write(&mut p.tx, Wire::MsgPack, &HostMsg::Tick { seq }).unwrap();
        let Some(ModeMsg::View { images, .. }) = codec::read(&mut p.rx, Wire::MsgPack, &mut p.buf).unwrap() else { panic!() };
        let h = Instant::now();
        match &images[0].pixels {
            Pixels::Slot(s) => tex.copy_from_slice(&ring.as_ref().unwrap().slot(*s).unwrap()[..FRAME]),
            Pixels::Inline(b) => tex.copy_from_slice(b),
            Pixels::Cached => panic!(),
        }
        assert_eq!(tex[FRAME - 1], (seq & 0xff) as u8, "frame {seq} has the right pixels");
        let done = Instant::now();
        rtt.push((done - t).as_secs_f64() * 1e3);
        host.push((done - h).as_secs_f64() * 1e3);
        if done > deadline {
            dropped += 1;
        } else {
            std::thread::sleep(deadline - done);
        }
    }
    codec::write(&mut p.tx, Wire::MsgPack, &HostMsg::Shutdown).unwrap();
    p.child.wait().unwrap();
    let r50 = pct(&mut rtt, 0.5);
    let r99 = pct(&mut rtt, 0.99);
    let h99 = pct(&mut host, 0.99);
    eprintln!("{} {n} frames: dropped {dropped}, rtt p50 {r50:.2} ms p99 {r99:.2} ms, host p99 {h99:.2} ms", if shm { "shm" } else { "inline" });
    (n, dropped, r50, r99, h99)
}

#[test]
fn actions_round_trip_through_a_persistent_child() {
    let (mut p, api) = spawn(&[]);
    accept(&mut p, api, None);
    for i in 0..100 {
        codec::write(&mut p.tx, Wire::MsgPack, &HostMsg::Action { id: format!("a{i}"), arg: i }).unwrap();
        let m: ModeMsg = codec::read(&mut p.rx, Wire::MsgPack, &mut p.buf).unwrap().unwrap();
        assert_eq!(m, ModeMsg::Effect(Effect::Say(format!("a{i}"))));
    }
    codec::write(&mut p.tx, Wire::MsgPack, &HostMsg::Shutdown).unwrap();
    assert!(p.child.wait().unwrap().success());
}

#[test]
fn incompatible_plugin_is_rejected_and_exits() {
    let (mut p, api) = spawn(&["--api", "90-91"]);
    let reason = negotiate(ApiRange::CURRENT, api).unwrap_err();
    codec::write(&mut p.tx, Wire::MsgPack, &Welcome::Rejected { reason }).unwrap();
    assert!(p.child.wait().unwrap().success());
}

#[test]
fn crash_is_seen_as_end_of_stream() {
    let (mut p, api) = spawn(&["--crash"]);
    accept(&mut p, api, None);
    let m: Option<ModeMsg> = codec::read(&mut p.rx, Wire::MsgPack, &mut p.buf).unwrap();
    assert!(m.is_none());
    assert!(!p.child.wait().unwrap().success());
}

/// Short CI version: 2 s each way; shared CI runners get some slack.
#[test]
fn frames_1080p_at_60fps() {
    let (n, dropped, ..) = run(true, 2);
    assert!(dropped * 20 <= n, "shm dropped {dropped} of {n}");
    let (n, dropped, ..) = run(false, 2);
    assert!(dropped * 10 <= n, "inline dropped {dropped} of {n}");
}

#[test]
#[ignore = "10 s benchmark: cargo test --release -- --ignored"]
fn benchmark_1080p_60fps_10s_shm() {
    let (_, dropped, r50, _, h99) = run(true, 10);
    assert_eq!(dropped, 0);
    assert!(r50 < 4.0 && h99 < 2.0, "rtt p50 {r50} ms, host p99 {h99} ms");
}
