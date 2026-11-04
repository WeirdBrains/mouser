//! A minimal mode plugin (see the crate docs): `--api MIN-MAX` sets the
//! versions it claims, `--crash` exits right after the handshake. It
//! listens to `frame.tick` (answers with a 1080p frame in the ring, or
//! inline) and `ui.action` (answers with a status message), and emits
//! `echo.pong` to itself on `echo.ping`.
use mouser_mode_api::codec::{self, Wire};
use mouser_mode_api::events::{EventDecl, Listener};
use mouser_mode_api::protocol::{ApiRange, Hello, ImageFormat, ImageRef, In, Out, Pixels, Provide, Welcome};
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
    let hello = Hello {
        api,
        name: "echo".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        provides: vec![Provide::Mode { hotkey: 'e', bindings: vec![], activate_on: vec![] }],
        listeners: ["frame.tick", "ui.action", "echo.*"].iter().map(|e| Listener::on(e)).collect(),
        emits: vec![EventDecl { name: "echo.pong".into(), schema: None }],
        wants_mcp: false,
        shm: true,
    };
    codec::write(&mut out, wire, &Out::Hello(hello)).unwrap();
    let mut ring = match codec::read::<Welcome>(&mut input, wire, &mut buf) {
        Ok(Some(Welcome::Accepted { shm, .. })) => shm.and_then(|s| Ring::open(&s).ok()),
        _ => return,
    };
    if crash {
        std::process::exit(3);
    }
    let (w, h) = (1920u32, 1080u32);
    let mut scratch = vec![0u8; (w * h * 4) as usize];
    while let Ok(Some(m)) = codec::read::<In>(&mut input, wire, &mut buf) {
        let reply = match m {
            In::Event(e) if e.name == "frame.tick" => {
                let seq = e.payload["n"].as_u64().unwrap_or(e.seq);
                let v = (seq & 0xff) as u8;
                let pixels = match ring.as_mut() {
                    Some(r) => {
                        let slot = (seq % r.slots() as u64) as u32;
                        r.slot_mut(slot).unwrap()[..scratch.len()].fill(v);
                        Pixels::Slot(slot)
                    }
                    None => {
                        scratch.fill(v);
                        Pixels::Inline { format: ImageFormat::Rgba, bytes: scratch.clone() }
                    }
                };
                let img = ImageRef { key: seq, width: w, height: h, src: (0, 0, w, h), rect: Rect::default(), pixels };
                Out::View { dock: None, panel: None, images: vec![img], fullscreen: true }
            }
            In::Event(e) if e.name == "ui.action" => Out::Effect(Effect::Say(e.payload["id"].as_str().unwrap_or("").into())),
            In::Event(e) if e.name == "echo.ping" => Out::Emit { name: "echo.pong".into(), payload: e.payload },
            In::Stopping { .. } => break,
            _ => continue,
        };
        if codec::write(&mut out, wire, &reply).is_err() {
            break;
        }
    }
}
