//! A minimal mode plugin (see the crate docs): `--api MIN-MAX` sets the
//! versions it claims, `--crash` exits right after the handshake.
use mouser_mode_api::codec::{self, Wire};
use mouser_mode_api::protocol::{ApiRange, Hello, HostMsg, ImageRef, ModeMsg, Pixels, Welcome};
use mouser_mode_api::{Effect, Rect, shm::Ring};
use std::io::{BufReader, BufWriter};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut api = ApiRange::CURRENT;
    if let Some(i) = args.iter().position(|a| a == "--api") {
        let (a, b) = args[i + 1].split_once('-').expect("--api MIN-MAX");
        api = ApiRange { min: a.parse().unwrap(), max: b.parse().unwrap() };
    }
    let crash = args.iter().any(|a| a == "--crash");
    let wire = Wire::from_env();
    let mut input = BufReader::new(std::io::stdin().lock());
    let mut out = BufWriter::new(std::io::stdout().lock());
    let mut buf = Vec::new();
    let hello =
        Hello { api, name: "echo".into(), version: env!("CARGO_PKG_VERSION").into(), hotkey: 'e', bindings: vec![], permissions: vec![], shm: true };
    codec::write(&mut out, wire, &ModeMsg::Hello(hello)).unwrap();
    let mut ring = match codec::read::<Welcome>(&mut input, wire, &mut buf) {
        Ok(Some(Welcome::Accepted { shm, .. })) => shm.and_then(|s| Ring::open(&s).ok()),
        _ => return,
    };
    if crash {
        std::process::exit(3);
    }
    let (w, h) = (1920u32, 1080u32);
    let mut scratch = vec![0u8; (w * h * 4) as usize];
    while let Ok(Some(m)) = codec::read::<HostMsg>(&mut input, wire, &mut buf) {
        let reply = match m {
            HostMsg::Tick { seq } => {
                let v = (seq & 0xff) as u8;
                let pixels = match ring.as_mut() {
                    Some(r) => {
                        let slot = (seq % r.slots() as u64) as u32;
                        r.slot_mut(slot).unwrap()[..scratch.len()].fill(v);
                        Pixels::Slot(slot)
                    }
                    None => {
                        scratch.fill(v);
                        Pixels::Inline(scratch.clone())
                    }
                };
                let img = ImageRef { key: seq, width: w, height: h, src: (0, 0, w, h), rect: Rect::default(), pixels };
                ModeMsg::View { dock: None, panel: None, images: vec![img], fullscreen: true }
            }
            HostMsg::Action { id, .. } => ModeMsg::Effect(Effect::Say(id)),
            HostMsg::Shutdown => break,
            _ => continue,
        };
        if codec::write(&mut out, wire, &reply).is_err() {
            break;
        }
    }
}
