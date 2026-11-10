use mouser_mode_api::codec::{self, Wire};
use mouser_mode_api::events::{self, EventDecl, Listener, RateLimit, may_emit, next_hops};
use mouser_mode_api::host_api::{Io, Spawn};
use mouser_mode_api::protocol::{ApiRange, In, negotiate};
use mouser_mode_api::services::allowed;
use mouser_mode_api::supervisor::{Decision, Restarts, WINDOW};
use mouser_mode_api::{Effect, shm::Ring};
use serde_json::json;
use std::time::{Duration, Instant};

#[test]
fn negotiation_picks_newest_shared_version() {
    let host = ApiRange { min: 1, max: 2 };
    assert_eq!(negotiate(host, ApiRange { min: 0, max: 5 }), Ok(2));
    assert_eq!(negotiate(host, ApiRange { min: 1, max: 1 }), Ok(1));
    assert!(negotiate(host, ApiRange { min: 0, max: 0 }).unwrap_err().contains("update the plugin"));
    assert!(negotiate(host, ApiRange { min: 3, max: 4 }).unwrap_err().contains("update Mouser"));
}

#[test]
fn api_1_is_frozen_and_the_draft_0_is_refused() {
    assert_eq!(mouser_mode_api::API_VERSION, 1);
    assert_eq!(ApiRange::CURRENT, ApiRange { min: 1, max: 1 });
    assert_eq!(negotiate(ApiRange::CURRENT, ApiRange { min: 0, max: 1 }), Ok(1));
    let why = negotiate(ApiRange::CURRENT, ApiRange { min: 0, max: 0 }).unwrap_err();
    assert!(why.contains("update the plugin"), "{why}");
}

#[test]
fn codec_round_trips_both_wires() {
    for wire in [Wire::MsgPack, Wire::Json] {
        let msgs = vec![
            In::Response { id: 1, result: Ok(json!({"a": [1, 2]})) },
            In::EffectResult { ok: false, refused: Some("no".into()) },
            In::Stopping { reason: "quit".into() },
        ];
        let mut bytes = Vec::new();
        for m in &msgs {
            codec::write(&mut bytes, wire, m).unwrap();
        }
        let mut r = bytes.as_slice();
        let mut buf = Vec::new();
        let mut back = Vec::new();
        while let Some(m) = codec::read::<In>(&mut r, wire, &mut buf).unwrap() {
            back.push(m);
        }
        assert_eq!(back, msgs);
    }
}

#[test]
fn codec_refuses_oversized_and_truncated_frames() {
    let mut r: &[u8] = &(u32::MAX).to_le_bytes();
    assert!(codec::read::<In>(&mut r, Wire::MsgPack, &mut Vec::new()).is_err());
    let mut r: &[u8] = &[5, 0, 0, 0, 1];
    assert!(codec::read::<In>(&mut r, Wire::MsgPack, &mut Vec::new()).is_err());
}

#[test]
fn restarts_back_off_then_give_up() {
    let t = Instant::now();
    let mut r = Restarts::new(3);
    assert_eq!(r.crashed(t), Decision::Restart(Duration::from_secs(1)));
    assert_eq!(r.crashed(t + Duration::from_secs(2)), Decision::Restart(Duration::from_secs(2)));
    assert_eq!(r.crashed(t + Duration::from_secs(5)), Decision::Restart(Duration::from_secs(4)));
    assert_eq!(r.crashed(t + Duration::from_secs(10)), Decision::GiveUp);
    assert!(r.gave_up());
    assert_eq!(r.crashed(t + Duration::from_secs(100)), Decision::GiveUp, "stays disabled");
    r.reset();
    assert_eq!(r.crashed(t + Duration::from_secs(101)), Decision::Restart(Duration::from_secs(1)));
}

#[test]
fn old_crashes_are_forgotten() {
    let t = Instant::now();
    let mut r = Restarts::new(1);
    assert!(matches!(r.crashed(t), Decision::Restart(_)));
    assert!(matches!(r.crashed(t + WINDOW + Duration::from_secs(1)), Decision::Restart(_)));
    let mut z = Restarts::new(0);
    assert_eq!(z.crashed(t), Decision::GiveUp, "max_restarts = 0 never restarts");
}

#[test]
fn permissions_gate_methods_and_effects() {
    let g = vec!["remote.read".to_string()];
    assert!(allowed("remote.list", &g));
    assert!(!allowed("agent.send", &g));
    assert!(allowed("prefs.get", &[]));
    assert!(!allowed("no.such", &g));
    assert_eq!(Effect::Type("ls".into()).permission(), Some("shell.type"));
    assert_eq!(Effect::Say("x".into()).permission(), None);
}

#[test]
fn ring_slots_are_shared_between_mappings() {
    let mut host = Ring::create("test", 2, 16).unwrap();
    let mut plugin = Ring::open(&host.spec()).unwrap();
    plugin.slot_mut(1).unwrap().fill(7);
    assert!(host.slot(1).unwrap().iter().all(|b| *b == 7));
    assert!(host.slot(2).is_none());
    host.slot_mut(0).unwrap()[0] = 1;
    assert_eq!(plugin.slot(0).unwrap()[0], 1);
    let path = host.spec().path;
    drop(plugin);
    drop(host);
    assert!(!std::path::Path::new(&path).exists(), "host removes the ring");
}

#[test]
fn listener_patterns() {
    assert!(Listener::on("input.*").matches("input.key"));
    assert!(!Listener::on("input.*").matches("inputx.key"));
    assert!(Listener::on("*").matches("anything"));
    assert!(Listener::on("shell.cwd_changed").matches("shell.cwd_changed"));
    assert!(!Listener::on("shell.cwd_changed").matches("shell.cwd"));
    assert!(Listener::on("x").hears_self(), "self-delivery is the default");
}

#[test]
fn emit_rules() {
    let decl = vec![EventDecl { name: "commander.selection".into(), schema: None }];
    assert!(may_emit("commander", &decl, "commander.selection", None, &[]).is_ok());
    assert!(may_emit("commander", &decl, "commander.other", None, &[]).unwrap_err().contains("declared"));
    assert!(may_emit("commander", &decl, "chat.reply_done", None, &[]).unwrap_err().contains("namespace"));
    for fact in ["shell.cwd_changed", "approval.result", "agent.stream", "tab.opened", "term.output"] {
        assert!(may_emit("commander", &decl, fact, None, &[]).is_err(), "{fact}");
    }
    assert!(may_emit("commander", &decl, "input.key", None, &[]).is_ok(), "into its own mode");
    assert!(may_emit("commander", &decl, "input.key", Some("chat"), &[]).is_err());
    assert!(may_emit("commander", &decl, "input.key", Some("chat"), &["events.inject:chat".into()]).is_ok());
}

#[test]
fn hop_limit_and_rate() {
    assert_eq!(next_hops(0), Some(1));
    assert_eq!(next_hops(events::MAX_HOPS - 1), Some(events::MAX_HOPS));
    assert_eq!(next_hops(events::MAX_HOPS), None);
    let t = Instant::now();
    let mut r = RateLimit::with(200.0, 1000.0, t);
    assert_eq!((0..1200).filter(|_| r.take(t)).count(), 1000, "burst of 1000");
    assert!(r.take(t + Duration::from_millis(10)), "refills at 200/s");
}

#[test]
fn escalation_only_in_the_visible_terminal() {
    let s = |p: &str, io| Spawn { program: p.into(), args: vec![], env_inherit: true, env_set: vec![], cwd: None, io, timeout_ms: None };
    assert!(s("sudo", Io::Pipes).check_escalation().is_err());
    assert!(s("/usr/bin/pkexec", Io::Pty { cols: 80, rows: 24 }).check_escalation().is_err());
    assert!(s("sudo", Io::VisiblePty { new: false }).check_escalation().is_ok());
    assert!(s("ffmpeg", Io::Pipes).check_escalation().is_ok());
}

/// The handshake as the published mode-api 0.2.0 decodes it (commander
/// 0.1.4, chat and preferences 0.1.2): `Context` without `reduce_motion`.
mod published_0_2_0 {
    use mouser_mode_api::{Rect, Viewport};
    #[derive(Debug, serde::Deserialize)]
    pub struct Context {
        pub viewport: Viewport,
        pub area: Rect,
    }
    #[derive(Debug, serde::Deserialize)]
    #[allow(dead_code)]
    pub struct Accepted {
        pub api: u32,
        pub session: String,
        pub ctx: Context,
        pub granted: Vec<String>,
        pub denied: Vec<(String, String)>,
        pub shm: Option<mouser_mode_api::protocol::ShmSpec>,
        pub mcp: Option<mouser_mode_api::protocol::McpGrant>,
        pub restore: Option<Vec<u8>>,
    }
    #[derive(Debug, serde::Deserialize)]
    pub enum Welcome {
        Accepted(Box<Accepted>),
        #[allow(dead_code)]
        Rejected {
            reason: String,
        },
    }
}

fn welcome_now() -> mouser_mode_api::protocol::Welcome {
    use mouser_mode_api::protocol::{Accepted, Welcome};
    use mouser_mode_api::{Context, Rect, Viewport};
    let ctx = Context {
        viewport: Viewport { width: 800, height: 600, cell_w: 8, cell_h: 16 },
        area: Rect { x: 0, y: 0, w: 800, h: 560 },
        reduce_motion: true,
    };
    Welcome::Accepted(Box::new(Accepted {
        api: 1,
        session: "commander-1".into(),
        ctx,
        granted: vec!["files.read".into()],
        denied: vec![("x.y".into(), "no".into())],
        shm: None,
        mcp: None,
        restore: Some(vec![1, 2, 3]),
    }))
}

#[test]
fn host_welcome_decodes_in_plugins_built_on_the_published_0_2_0() {
    // The bug: 1.4.5 sent Context as a 3-element array and every published
    // plugin exited with "array had incorrect length, expected 2".
    let mut array = Vec::new();
    codec::write(&mut array, Wire::MsgPack, &welcome_now()).unwrap();
    let err = codec::read::<published_0_2_0::Welcome>(&mut array.as_slice(), Wire::MsgPack, &mut Vec::new()).unwrap_err();
    assert!(err.to_string().contains("incorrect length"), "{err}");

    let mut named = Vec::new();
    codec::write_named(&mut named, Wire::MsgPack, &welcome_now()).unwrap();
    let Some(published_0_2_0::Welcome::Accepted(a)) = codec::read(&mut named.as_slice(), Wire::MsgPack, &mut Vec::new()).unwrap() else {
        panic!("not accepted")
    };
    assert_eq!((a.ctx.viewport.width, a.ctx.area.h, a.session.as_str()), (800, 560, "commander-1"));
    assert_eq!(a.restore.as_deref(), Some(&[1u8, 2, 3][..]));
}

#[test]
fn named_host_messages_round_trip_into_the_current_crate() {
    let mut named = Vec::new();
    codec::write_named(&mut named, Wire::MsgPack, &welcome_now()).unwrap();
    let back: mouser_mode_api::protocol::Welcome = codec::read(&mut named.as_slice(), Wire::MsgPack, &mut Vec::new()).unwrap().unwrap();
    assert_eq!(back, welcome_now());
    let msgs = vec![
        In::Event(events::Event {
            seq: 7,
            ts_us: 1,
            tab: Some(0),
            name: "view.resized".into(),
            payload: json!({"reduce_motion": true}),
            origin: events::Origin::Plugin { name: "chat".into(), session: "s".into() },
            cause: Some(3),
            hops: 1,
            synthetic: true,
        }),
        In::Response { id: 1, result: Err("no".into()) },
        In::Stopping { reason: "quit".into() },
    ];
    let mut bytes = Vec::new();
    for m in &msgs {
        codec::write_named(&mut bytes, Wire::MsgPack, m).unwrap();
    }
    let (mut r, mut buf, mut got) = (bytes.as_slice(), Vec::new(), Vec::new());
    while let Some(m) = codec::read::<In>(&mut r, Wire::MsgPack, &mut buf).unwrap() {
        got.push(m);
    }
    assert_eq!(got, msgs);
}
