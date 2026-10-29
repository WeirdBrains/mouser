use mouser_core::compression::{CompressionAlgo, CompressionConfig};
use mouser_core::scrollback::{Scrollback, ScrollbackConfig};
use std::thread;
use std::time::Duration;

#[test]
fn compression_roundtrip_zstd_store() {
    let cfg = ScrollbackConfig {
        max_scrollback_lines: 10_000,
        chunk_lines: 4,
        idle_compress_ms: 50,
        prefetch_chunks: 1,
        compression: CompressionConfig { algorithm: CompressionAlgo::Zstd, level: 3 },
    };
    let sb = Scrollback::new(cfg);
    for i in 0..12 {
        sb.push_line(format!("line-{i}"));
    }
    // allow idle and compress
    thread::sleep(Duration::from_millis(60));
    sb.maybe_compress_idle();
    // Now fetch lines spanning multiple chunks (forces decompression)
    let got = sb.get_lines(0, 12);
    assert_eq!(got.len(), 12);
    assert_eq!(got[0], "line-0");
    assert_eq!(got[11], "line-11");
}

#[test]
fn search_across_compressed_chunks() {
    let cfg = ScrollbackConfig {
        max_scrollback_lines: 10_000,
        chunk_lines: 3,
        idle_compress_ms: 10,
        prefetch_chunks: 1,
        compression: CompressionConfig { algorithm: CompressionAlgo::Zstd, level: 3 },
    };
    let sb = Scrollback::new(cfg);
    for i in 0..15 {
        sb.push_line(format!("alpha beta gamma {i}"));
    }
    thread::sleep(Duration::from_millis(15));
    sb.maybe_compress_idle();
    let results = sb.search("beta", &Default::default());
    assert!(results.len() >= 15);
}
