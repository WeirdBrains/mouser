//! Feature negotiation with the real gallery program on the protocol's own
//! supervisor: a host that offers `views.1` / `media.1` gets `Hosted`, and a
//! host that offers nothing (Mouser before 1.6, which can't read `Hosted`)
//! gets only the plugin's own view with its notice.

use mouser_mode_api::host::{Grants, Spec, Supervisor};
use mouser_mode_api::hosted::{FEATURE_MEDIA, FEATURE_VIEWS};
use mouser_mode_api::protocol::Out;
use mouser_mode_api::{Context, Rect, Viewport};
use std::time::{Duration, Instant};

fn run(features: Vec<String>) -> Vec<Out> {
    let spec = Spec { name: "gallery".into(), program: env!("CARGO_BIN_EXE_mouser-gallery").into(), args: vec![], env: vec![], shm: None, features };
    let ctx = Context {
        viewport: Viewport { width: 800, height: 600, cell_w: 8, cell_h: 16 },
        area: Rect { x: 0, y: 0, w: 800, h: 600 },
        reduce_motion: true,
    };
    let mut s = Supervisor::new(spec, ctx, 0, |_| Grants::default());
    let mut out = Vec::new();
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end && !out.iter().any(|m| matches!(m, Out::View { .. })) {
        out.extend(s.poll(Instant::now()));
        std::thread::sleep(Duration::from_millis(5));
    }
    // Anything sent along with the first view.
    std::thread::sleep(Duration::from_millis(100));
    out.extend(s.poll(Instant::now()));
    s.stop("test");
    out
}

fn texts(out: &[Out]) -> Vec<String> {
    out.iter()
        .filter_map(|m| match m {
            Out::View { dock: Some(d), .. } => Some(d.items.iter().map(|i| i.text.clone()).collect::<Vec<_>>()),
            _ => None,
        })
        .flatten()
        .collect()
}

#[test]
fn an_older_host_gets_no_hosted_message() {
    let out = run(vec![]);
    assert!(!out.iter().any(|m| matches!(m, Out::Hosted(_))), "{out:?}");
    assert!(texts(&out).iter().any(|t| t.contains("needs Mouser 1.6")), "{:?}", texts(&out));
}

#[test]
fn a_host_offering_views_gets_them() {
    let out = run(vec![FEATURE_VIEWS.into(), FEATURE_MEDIA.into()]);
    assert!(out.iter().any(|m| matches!(m, Out::Hosted(v) if v.iter().any(|h| h.id == "files"))), "{out:?}");
    assert!(!texts(&out).iter().any(|t| t.contains("needs Mouser 1.6")));
}
