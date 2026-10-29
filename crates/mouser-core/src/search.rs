use crate::scrollback::Scrollback;
use regex::RegexBuilder;

#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    pub case_sensitive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub line_idx: usize,
    pub byte_start: usize,
    pub byte_end: usize,
}

impl Scrollback {
    pub fn search(&self, pattern: &str, opts: &SearchOptions) -> Vec<SearchResult> {
        // Search entire scrollback (chunks + tail)
        let re = RegexBuilder::new(pattern).case_insensitive(!opts.case_sensitive).build();
        let re = match re {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        let total = self.total_lines();
        let mut out = Vec::new();
        // Fetch in windows to avoid holding a huge buffer
        let window = 4096usize;
        let mut idx = 0usize;
        while idx < total {
            let count = (total - idx).min(window);
            let lines = self.get_lines(idx, count);
            for (i, line) in lines.iter().enumerate() {
                for m in re.find_iter(line) {
                    out.push(SearchResult { line_idx: idx + i, byte_start: m.start(), byte_end: m.end() });
                }
            }
            idx += count;
        }
        out
    }
}
