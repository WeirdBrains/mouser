//! The host supervisor against a real plugin process: restarts back off,
//! stop after `max_restarts`, the restart action resets them, saved state
//! comes back after a crash, and an incompatible plugin stays off.
use mouser_mode_api::events::{Event, Origin};
use mouser_mode_api::host::{Grants, Spec, State, Supervisor};
use mouser_mode_api::protocol::{In, Out};
use mouser_mode_api::supervisor::Restarts;
use mouser_mode_api::{Context, Effect, Rect, Viewport};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

fn spec(args: &[&str]) -> Spec {
    Spec {
        name: "echo".into(),
        program: env!("CARGO_BIN_EXE_mouser-mode-echo").into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        env: vec![],
        shm: None,
        features: vec![],
    }
}

fn ctx() -> Context {
    Context { viewport: Viewport { width: 800, height: 600, cell_w: 8, cell_h: 16 }, area: Rect { x: 0, y: 0, w: 800, h: 600 }, reduce_motion: false }
}

fn ev(name: &str, payload: Value) -> In {
    In::Event(Event { seq: 1, ts_us: 0, tab: None, name: name.into(), payload, origin: Origin::Host, cause: None, hops: 0, synthetic: false })
}

/// Poll until `f` matches a message or the state, for up to 5 s.
fn wait(s: &mut Supervisor, mut f: impl FnMut(&Supervisor, &[Out]) -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        let out = s.poll(Instant::now());
        if f(s, &out) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out in state {:?}", s.state());
}

#[test]
fn runs_and_answers() {
    let mut s = Supervisor::new(spec(&[]), ctx(), 3, |_| Grants::default());
    assert_eq!(s.state(), &State::Running);
    assert_eq!(s.connection().unwrap().hello.name, "echo");
    assert!(s.send(&ev("ui.action", json!({"id": "hi"})), Instant::now()));
    wait(&mut s, |_, out| out.contains(&Out::Effect(Effect::Say("hi".into()))));
    s.stop("test");
}

#[test]
fn crash_loop_backs_off_then_gives_up() {
    let unit = Duration::from_millis(40);
    let mut s = Supervisor::with_restarts(spec(&["--crash"]), ctx(), Restarts::with_unit(3, unit), |_| Grants::default());
    let mut waits = Vec::new();
    let mut last = None;
    wait(&mut s, |s, _| {
        if let State::Waiting { until, crashes } = s.state()
            && last != Some(*crashes)
        {
            last = Some(*crashes);
            waits.push(until.saturating_duration_since(Instant::now()));
        }
        matches!(s.state(), State::Disabled { .. })
    });
    assert_eq!(waits.len(), 3, "3 restarts, then it gives up: {waits:?}");
    assert!(waits[1] > waits[0] && waits[2] > waits[1], "waits double: {waits:?}");
    assert!(waits[2] <= unit * 4);
    let line = s.status(Instant::now()).unwrap();
    assert!(line.contains("after 4 crashes") && line.contains("restart"), "{line}");
    std::thread::sleep(unit * 10);
    s.poll(Instant::now());
    assert!(matches!(s.state(), State::Disabled { .. }), "stays off");
    // The restart action counts from zero again.
    s.restart(Instant::now());
    wait(&mut s, |s, _| !matches!(s.state(), State::Running));
    assert!(matches!(s.state(), State::Waiting { crashes: 1, .. }), "{:?}", s.state());
}

#[test]
fn saved_state_comes_back_after_a_crash() {
    let mut s = Supervisor::with_restarts(spec(&[]), ctx(), Restarts::with_unit(3, Duration::from_millis(10)), |_| Grants::default());
    s.send(&ev("echo.save", json!({"state": "cwd=/tmp"})), Instant::now());
    std::thread::sleep(Duration::from_millis(100));
    s.poll(Instant::now());
    // Kill it behind the supervisor's back.
    s.send(&In::Stopping { reason: "simulated crash".into() }, Instant::now());
    wait(&mut s, |_, out| out.iter().any(|m| matches!(m, Out::Emit { name, payload } if name == "echo.restored" && payload["state"] == "cwd=/tmp")));
    assert_eq!(s.state(), &State::Running);
    s.stop("test");
}

#[test]
fn incompatible_plugin_stays_off() {
    let s = Supervisor::new(spec(&["--api", "90-91"]), ctx(), 3, |_| Grants::default());
    let State::Disabled { reason } = s.state() else { panic!("{:?}", s.state()) };
    assert!(reason.contains("update Mouser"), "{reason}");
}

#[test]
fn draft_api_0_plugin_is_told_to_update() {
    let s = Supervisor::new(spec(&["--api", "0-0"]), ctx(), 3, |_| Grants::default());
    let State::Disabled { reason } = s.state() else { panic!("{:?}", s.state()) };
    assert!(reason.contains("update the plugin"), "{reason}");
}

#[test]
fn missing_binary_is_not_fatal() {
    let mut sp = spec(&[]);
    sp.program = "/nonexistent/mouser-plugin".into();
    let s = Supervisor::with_restarts(sp, ctx(), Restarts::with_unit(0, Duration::from_millis(1)), |_| Grants::default());
    assert!(matches!(s.state(), State::Disabled { .. }));
}
