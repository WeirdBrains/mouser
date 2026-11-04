//! A minimal mode plugin built on [`mouser_mode_api::serve`], used by the
//! tests and benchmarks. `--api MIN-MAX` sets the versions it claims,
//! `--crash` exits right after the handshake. It listens to `frame.tick`
//! (answers with a 1080p frame in the ring, or inline), `ui.action`
//! (answers with a status message) and `echo.*`: `echo.ping` emits
//! `echo.pong`, `echo.save` saves its payload as state, and after a restart
//! it emits `echo.restored` with what it got back.
use mouser_mode_api::events::{EventDecl, Listener};
use mouser_mode_api::protocol::{Accepted, ApiRange, Handled, Hello, ImageFormat, ImageRef, Pixels, Provide};
use mouser_mode_api::{Effect, Host, Mode, Rect, events::Event};
use serde_json::json;

struct Echo {
    api: ApiRange,
    crash: bool,
    scratch: Vec<u8>,
}

const W: u32 = 1920;
const H: u32 = 1080;

impl Mode for Echo {
    fn hello(&self) -> Hello {
        Hello {
            api: self.api,
            name: "echo".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            provides: vec![Provide::Mode { hotkey: 'e', bindings: vec![], activate_on: vec![] }],
            listeners: ["frame.tick", "ui.action", "echo.*"].iter().map(|e| Listener::on(e)).collect(),
            emits: ["echo.pong", "echo.restored"].iter().map(|n| EventDecl { name: n.to_string(), schema: None }).collect(),
            wants_mcp: false,
            shm: true,
        }
    }

    fn welcome(&mut self, w: &Accepted, host: &Host) {
        if self.crash {
            std::process::exit(3);
        }
        if let Some(r) = &w.restore {
            let _ = host.emit("echo.restored", json!({ "state": String::from_utf8_lossy(r) }));
        }
    }

    fn event(&mut self, e: &Event, host: &Host) -> Handled {
        let _ = match e.name.as_str() {
            "frame.tick" => {
                let seq = e.payload["n"].as_u64().unwrap_or(e.seq);
                let v = (seq & 0xff) as u8;
                let n = self.scratch.len();
                let slot = (seq % 2) as u32;
                let pixels = match host.with_slot(slot, |s| s[..n].fill(v)) {
                    Some(()) => Pixels::Slot(slot),
                    None => {
                        self.scratch.fill(v);
                        Pixels::Inline { format: ImageFormat::Rgba, bytes: self.scratch.clone() }
                    }
                };
                host.view(None, None, vec![ImageRef { key: seq, width: W, height: H, src: (0, 0, W, H), rect: Rect::default(), pixels }], true)
            }
            "ui.action" => host.effect(Effect::Say(e.payload["id"].as_str().unwrap_or("").into())),
            "echo.ping" => host.emit("echo.pong", e.payload.clone()),
            "echo.save" => host.save_state(e.payload["state"].as_str().unwrap_or("").as_bytes().to_vec()),
            _ => Ok(()),
        };
        Handled::Handled
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut api = ApiRange::CURRENT;
    if let Some(i) = args.iter().position(|a| a == "--api") {
        let (a, b) = args[i + 1].split_once('-').expect("--api MIN-MAX");
        api = ApiRange { min: a.parse().unwrap(), max: b.parse().unwrap() };
    }
    let crash = args.iter().any(|a| a == "--crash");
    let _ = mouser_mode_api::serve(Echo { api, crash, scratch: vec![0; (W * H * 4) as usize] });
}
