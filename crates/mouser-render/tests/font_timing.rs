//! Timing probe (ignored by default):
//! `cargo test --release -p mouser-render --test font_timing -- --ignored --nocapture`
use mouser_render::text::{FontStack, system_db};
use std::time::Instant;

#[test]
#[ignore]
fn font_load_timings() {
    let t = Instant::now();
    let db = system_db();
    println!("system_db scan: {:?}", t.elapsed());
    for fam in [None, Some("J"), Some("Ja"), Some("Fira Code"), Some("Liberation Mono")] {
        let t = Instant::now();
        let f = FontStack::from_db(&db, fam, 15.0);
        println!("from_db({fam:?}): {:?} ({} faces)", t.elapsed(), f.font_count());
    }
    let f = FontStack::from_db(&db, None, 15.0);
    let t = Instant::now();
    let _g = f.resized(16.0);
    println!("resized: {:?}", t.elapsed());
}
