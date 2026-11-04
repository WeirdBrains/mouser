use mouser_mode_api::codec::{self, Wire};
use mouser_mode_api::protocol::{ApiRange, HostMsg, negotiate};
use mouser_mode_api::services::allowed;
use mouser_mode_api::supervisor::{Decision, Restarts, WINDOW};
use mouser_mode_api::{Effect, shm::Ring};
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
fn codec_round_trips_both_wires() {
    for wire in [Wire::MsgPack, Wire::Json] {
        let msgs = vec![HostMsg::Action { id: "open".into(), arg: 7 }, HostMsg::Text(b"hi\x1b".to_vec()), HostMsg::Tick { seq: 9 }];
        let mut bytes = Vec::new();
        for m in &msgs {
            codec::write(&mut bytes, wire, m).unwrap();
        }
        let mut r = bytes.as_slice();
        let mut buf = Vec::new();
        let mut back = Vec::new();
        while let Some(m) = codec::read::<HostMsg>(&mut r, wire, &mut buf).unwrap() {
            back.push(m);
        }
        assert_eq!(back, msgs);
    }
}

#[test]
fn codec_refuses_oversized_and_truncated_frames() {
    let mut r: &[u8] = &(u32::MAX).to_le_bytes();
    assert!(codec::read::<HostMsg>(&mut r, Wire::MsgPack, &mut Vec::new()).is_err());
    let mut r: &[u8] = &[5, 0, 0, 0, 1];
    assert!(codec::read::<HostMsg>(&mut r, Wire::MsgPack, &mut Vec::new()).is_err());
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
