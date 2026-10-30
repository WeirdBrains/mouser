use crate::compression::{CompressionAlgo, CompressionConfig, compress_bytes, decompress_bytes};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrollbackConfig {
    pub max_scrollback_lines: usize,
    pub chunk_lines: usize,
    pub idle_compress_ms: u64,
    pub prefetch_chunks: usize,
    #[serde(default)]
    pub compression: CompressionConfig,
}

impl Default for ScrollbackConfig {
    fn default() -> Self {
        Self {
            max_scrollback_lines: 200_000,
            chunk_lines: 1024,
            idle_compress_ms: 15_000,
            prefetch_chunks: 1,
            compression: CompressionConfig::default(),
        }
    }
}

#[derive(Debug)]
pub enum ScrollbackChunk {
    Uncompressed { lines: Vec<String>, last_access: Instant },
    Compressed { data: Vec<u8>, last_access: Instant, line_count: usize, algo: CompressionAlgo },
}

impl ScrollbackChunk {
    fn line_count(&self) -> usize {
        match self {
            ScrollbackChunk::Uncompressed { lines, .. } => lines.len(),
            ScrollbackChunk::Compressed { line_count, .. } => *line_count,
        }
    }
}

#[derive(Debug)]
pub struct Scrollback {
    cfg: ScrollbackConfig,
    chunks: RwLock<Vec<ScrollbackChunk>>,
    tail: RwLock<Vec<String>>, // recent, always uncompressed
}

impl Scrollback {
    pub fn new(cfg: ScrollbackConfig) -> Self {
        Self { cfg, chunks: RwLock::new(Vec::new()), tail: RwLock::new(Vec::new()) }
    }

    pub fn push_line(&self, line: String) {
        // First, append to tail
        let maybe_full_chunk = {
            let mut tail = self.tail.write();
            tail.push(line);
            if tail.len() >= self.cfg.chunk_lines { Some(std::mem::take(&mut *tail)) } else { None }
        };
        // If a chunk was produced, append it to chunks and enforce limits
        if let Some(lines) = maybe_full_chunk {
            let mut chunks = self.chunks.write();
            chunks.push(ScrollbackChunk::Uncompressed { lines, last_access: Instant::now() });
            self.enforce_max_locked(&mut chunks);
        }
    }

    /// Drop every saved line (ED 3).
    pub fn clear(&self) {
        self.chunks.write().clear();
        self.tail.write().clear();
    }

    pub fn total_lines(&self) -> usize {
        let chunks = self.chunks.read();
        let chunk_lines: usize = chunks.iter().map(|c| c.line_count()).sum();
        chunk_lines + self.tail.read().len()
    }

    fn enforce_max_locked(&self, chunks: &mut Vec<ScrollbackChunk>) {
        // Drop oldest chunks when over limit; keep tail always.
        let mut total = chunks.iter().map(|c| c.line_count()).sum::<usize>() + self.tail.read().len();
        while total > self.cfg.max_scrollback_lines && !chunks.is_empty() {
            let dropped = chunks.remove(0).line_count();
            total = total.saturating_sub(dropped);
        }
    }

    pub fn get_lines(&self, start: usize, count: usize) -> Vec<String> {
        // Materialize a range across chunks + tail.
        let mut out = Vec::with_capacity(count);
        let mut remaining = count;
        let mut idx = 0usize;
        {
            let mut chunks = self.chunks.write(); // take write to update last_access
            for chunk in chunks.iter_mut() {
                let ln = chunk.line_count();
                if start < idx + ln && remaining > 0 {
                    // We need from this chunk
                    let offset = start.saturating_sub(idx);
                    let needed = remaining.min(ln - offset);
                    let lines = Self::materialize_chunk(chunk, &self.cfg.compression);
                    out.extend_from_slice(&lines[offset..offset + needed]);
                    remaining -= needed;
                    chunk_touch(chunk);
                }
                idx += ln;
                if remaining == 0 {
                    break;
                }
            }
        } // drop chunks lock
        if remaining > 0 {
            let tail = self.tail.read();
            if start < idx + tail.len() {
                let offset = start.saturating_sub(idx);
                let needed = remaining.min(tail.len().saturating_sub(offset));
                out.extend_from_slice(&tail[offset..offset + needed]);
            }
        }
        out
    }

    fn materialize_chunk<'a>(chunk: &'a mut ScrollbackChunk, cfg: &CompressionConfig) -> &'a Vec<String> {
        match chunk {
            ScrollbackChunk::Uncompressed { lines, .. } => lines,
            ScrollbackChunk::Compressed { data, last_access, line_count, algo: _ } => {
                let bytes = decompress_bytes(cfg, data).expect("decompress");
                let text = String::from_utf8(bytes).expect("utf8");
                let lines: Vec<String> = text.split('\n').take(*line_count).map(|s| s.to_string()).collect();
                *chunk = ScrollbackChunk::Uncompressed { lines, last_access: *last_access };
                if let ScrollbackChunk::Uncompressed { lines, .. } = chunk { lines } else { unreachable!() }
            }
        }
    }

    pub fn maybe_compress_idle(&self) {
        let idle = Duration::from_millis(self.cfg.idle_compress_ms);
        let mut chunks = self.chunks.write();
        for chunk in chunks.iter_mut() {
            match chunk {
                ScrollbackChunk::Uncompressed { lines, last_access } => {
                    if last_access.elapsed() >= idle && !lines.is_empty() && self.cfg.compression.algorithm != CompressionAlgo::None {
                        let text = lines.join("\n");
                        let data = compress_bytes(&self.cfg.compression, text.as_bytes()).expect("compress");
                        let count = lines.len();
                        *chunk = ScrollbackChunk::Compressed {
                            data,
                            last_access: Instant::now(),
                            line_count: count,
                            algo: self.cfg.compression.algorithm,
                        };
                    }
                }
                ScrollbackChunk::Compressed { .. } => { /* already compressed */ }
            }
        }
    }

    pub fn prefetch_around(&self, first_line: usize, last_line: usize) {
        // Decompress ±prefetch_chunks around the viewport lines.
        let mut chunks = self.chunks.write();
        let mut idx = 0usize;
        let mut needed_indices = Vec::new();
        for (ci, chunk) in chunks.iter().enumerate() {
            let ln = chunk.line_count();
            let chunk_end = idx + ln;
            if chunk_end > first_line && idx < last_line {
                needed_indices.push(ci);
            }
            idx += ln;
        }
        // Expand by prefetch window
        let pf = self.cfg.prefetch_chunks;
        let mut to_materialize = Vec::new();
        for &ci in &needed_indices {
            let start = ci.saturating_sub(pf);
            let end = (ci + pf + 1).min(chunks.len());
            for j in start..end {
                to_materialize.push(j);
            }
        }
        to_materialize.sort_unstable();
        to_materialize.dedup();
        for j in to_materialize {
            let _ = Self::materialize_chunk(&mut chunks[j], &self.cfg.compression);
            chunk_touch(&mut chunks[j]);
        }
    }
}

fn chunk_touch(chunk: &mut ScrollbackChunk) {
    match chunk {
        ScrollbackChunk::Uncompressed { last_access, .. } => *last_access = Instant::now(),
        ScrollbackChunk::Compressed { last_access, .. } => *last_access = Instant::now(),
    }
}
