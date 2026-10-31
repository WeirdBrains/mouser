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
    /// Soft-wrap flag per stored line (same order and length as the lines).
    wrapped: RwLock<std::collections::VecDeque<bool>>,
}

impl Scrollback {
    pub fn new(cfg: ScrollbackConfig) -> Self {
        Self { cfg, chunks: RwLock::new(Vec::new()), tail: RwLock::new(Vec::new()), wrapped: RwLock::new(Default::default()) }
    }

    pub fn push_line(&self, line: String) {
        self.push_line_wrapped(line, false);
    }

    /// Whether stored line `i` continues on line `i + 1` (soft wrap).
    pub fn is_wrapped(&self, i: usize) -> bool {
        self.wrapped.read().get(i).copied().unwrap_or(false)
    }

    pub fn push_line_wrapped(&self, line: String, wrapped: bool) {
        self.wrapped.write().push_back(wrapped);
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

    /// Every saved line with its soft-wrap flag, oldest first.
    pub fn all_lines_wrapped(&self) -> Vec<(String, bool)> {
        let lines = self.get_lines(0, self.total_lines());
        let w = self.wrapped.read();
        lines.into_iter().enumerate().map(|(i, l)| (l, w.get(i).copied().unwrap_or(false))).collect()
    }

    /// Start of the logical line holding line `i` (the first of its soft-wrapped rows).
    pub fn line_start(&self, i: usize) -> usize {
        let w = self.wrapped.read();
        let mut i = i.min(w.len());
        while i > 0 && w.get(i - 1).copied().unwrap_or(false) {
            i -= 1;
        }
        i
    }

    /// Lines `a..b` with their soft-wrap flags.
    pub fn lines_wrapped(&self, a: usize, b: usize) -> Vec<(String, bool)> {
        let lines = self.get_lines(a, b.saturating_sub(a));
        let w = self.wrapped.read();
        lines.into_iter().enumerate().map(|(i, l)| (l, w.get(a + i).copied().unwrap_or(false))).collect()
    }

    /// Replace lines `a..b` with `new`. Only the chunks touching the range are
    /// rebuilt. Returns how many of the oldest lines were dropped to stay within
    /// the limit.
    pub fn splice(&self, a: usize, b: usize, new: Vec<(String, bool)>) -> usize {
        let mut chunks = self.chunks.write();
        let mut tail = self.tail.write();
        let total = chunks.iter().map(|c| c.line_count()).sum::<usize>() + tail.len();
        let (a, b) = (a.min(total), b.min(total).max(a.min(total)));
        // First chunk touching `a`.
        let (mut ci, mut idx) = (0, 0);
        while ci < chunks.len() && idx + chunks[ci].line_count() <= a {
            idx += chunks[ci].line_count();
            ci += 1;
        }
        let start = idx;
        let mut gathered: Vec<String> = Vec::new();
        let mut cj = ci;
        while cj < chunks.len() && idx < b.max(a + 1) {
            gathered.extend(Self::materialize_chunk(&mut chunks[cj], &self.cfg.compression).iter().cloned());
            idx += chunks[cj].line_count();
            cj += 1;
        }
        let with_tail = cj == chunks.len();
        if with_tail {
            gathered.append(&mut tail);
        }
        let (nw, new_lines): (Vec<bool>, Vec<String>) = new.into_iter().map(|(l, w)| (w, l)).unzip();
        let after = gathered.split_off((b - start).min(gathered.len()));
        gathered.truncate(a - start);
        gathered.extend(new_lines);
        gathered.extend(after);
        let step = self.cfg.chunk_lines.max(1);
        let mut rebuilt = Vec::new();
        let mut it = gathered.into_iter().peekable();
        while it.peek().is_some() {
            let part: Vec<String> = it.by_ref().take(step).collect();
            if with_tail && it.peek().is_none() && part.len() < step {
                *tail = part;
                break;
            }
            rebuilt.push(ScrollbackChunk::Uncompressed { lines: part, last_access: Instant::now() });
        }
        chunks.splice(ci..cj, rebuilt);
        {
            let mut w = self.wrapped.write();
            let len = w.len();
            let rest = w.split_off(b.min(len));
            w.truncate(a);
            w.extend(nw);
            w.extend(rest);
        }
        drop(tail);
        let before = chunks.iter().map(|c| c.line_count()).sum::<usize>() + self.tail.read().len();
        self.enforce_max_locked(&mut chunks);
        let after_n = chunks.iter().map(|c| c.line_count()).sum::<usize>() + self.tail.read().len();
        before - after_n
    }

    /// Replace every saved line (re-wrapping). Returns how many lines were dropped
    /// from the front to stay within the limit.
    pub fn replace_all(&self, lines: Vec<(String, bool)>) -> usize {
        let max = self.cfg.max_scrollback_lines;
        let drop = lines.len().saturating_sub(max);
        let step = self.cfg.chunk_lines.max(1);
        let mut chunks = Vec::new();
        let mut cur = Vec::with_capacity(step);
        let mut wrapped = std::collections::VecDeque::with_capacity(lines.len() - drop);
        for (l, w) in lines.into_iter().skip(drop) {
            wrapped.push_back(w);
            cur.push(l);
            if cur.len() == step {
                chunks.push(ScrollbackChunk::Uncompressed {
                    lines: std::mem::replace(&mut cur, Vec::with_capacity(step)),
                    last_access: Instant::now(),
                });
            }
        }
        *self.chunks.write() = chunks;
        *self.tail.write() = cur;
        *self.wrapped.write() = wrapped;
        drop
    }

    /// Drop every saved line (ED 3).
    pub fn clear(&self) {
        self.chunks.write().clear();
        self.tail.write().clear();
        self.wrapped.write().clear();
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
            let mut w = self.wrapped.write();
            let n = dropped.min(w.len());
            w.drain(..n);
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

    /// Compress at most one idle chunk (called often, e.g. on every PTY read).
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
                        // One chunk per call: the lock is shared with drawing and
                        // re-wrapping, so never hold it for the whole scrollback.
                        return;
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
