//! Small in-house fuzzy matcher for pickers (font families, ...).
//!
//! A query matches when its characters appear in order in the candidate
//! (case-insensitive). Matches score higher when they land on word starts and
//! form contiguous runs; gaps cost a little.

const NEG: i32 = i32::MIN / 4;
const MATCH: i32 = 16;
const CONTIGUOUS: i32 = 24;
const GAP: i32 = 1;
const LEADING_GAP_MAX: i32 = 8;

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn boundary_bonus(chars: &[char], j: usize) -> i32 {
    if j == 0 {
        return 32;
    }
    let (p, c) = (chars[j - 1], chars[j]);
    if !p.is_alphanumeric() {
        28
    } else if p.is_lowercase() && c.is_uppercase() || p.is_alphabetic() && c.is_numeric() {
        24
    } else {
        0
    }
}

/// Score `candidate` against `query`; returns the score and the matched
/// character positions (char indices), or `None` if it doesn't match.
/// An empty query matches everything with score 0.
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<(i32, Vec<usize>)> {
    let q: Vec<char> = query.chars().map(lower).collect();
    if q.is_empty() {
        return Some((0, Vec::new()));
    }
    let chars: Vec<char> = candidate.chars().collect();
    let lc: Vec<char> = chars.iter().copied().map(lower).collect();
    let (n, m) = (q.len(), lc.len());
    if n > m {
        return None;
    }
    // score[i][j]: best score with q[i] matched at j; from[i][j]: where q[i-1] was.
    let mut score = vec![vec![NEG; m]; n];
    let mut from = vec![vec![usize::MAX; m]; n];
    for i in 0..n {
        // Best predecessor at k <= j-2 (a gap of at least one char), decayed per char.
        let (mut run, mut run_from) = (NEG, usize::MAX);
        for j in 0..m {
            if i > 0 && j >= 2 {
                run -= GAP;
                let v = score[i - 1][j - 2];
                if v - GAP > run {
                    run = v - GAP;
                    run_from = j - 2;
                }
            }
            if lc[j] != q[i] {
                continue;
            }
            let bonus = MATCH + boundary_bonus(&chars, j);
            if i == 0 {
                score[i][j] = bonus - (j as i32).min(LEADING_GAP_MAX);
                continue;
            }
            if j >= 1 && score[i - 1][j - 1] > NEG {
                let v = score[i - 1][j - 1] + bonus + CONTIGUOUS;
                if v > score[i][j] {
                    score[i][j] = v;
                    from[i][j] = j - 1;
                }
            }
            if run > NEG {
                let v = run + bonus;
                if v > score[i][j] {
                    score[i][j] = v;
                    from[i][j] = run_from;
                }
            }
        }
    }
    let (mut j, best) = (0..m).map(|j| (j, score[n - 1][j])).max_by_key(|&(j, s)| (s, std::cmp::Reverse(j)))?;
    if best <= NEG {
        return None;
    }
    let mut pos = vec![0; n];
    for i in (0..n).rev() {
        pos[i] = j;
        j = from[i][j];
    }
    let mut total = best;
    if lc == q {
        total += 100;
    }
    Some((total, pos))
}

/// A matching item: index into the input list and matched char positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filtered {
    pub index: usize,
    pub positions: Vec<usize>,
}

/// Items matching `query`, always in name order (case-insensitive) whatever
/// the match quality: the query only filters. An empty query keeps everything.
pub fn filter_sorted<S: AsRef<str>>(query: &str, items: &[S]) -> Vec<Filtered> {
    let mut out: Vec<Filtered> = items
        .iter()
        .enumerate()
        .filter_map(|(index, s)| fuzzy_match(query, s.as_ref()).map(|(_, positions)| Filtered { index, positions }))
        .collect();
    out.sort_by(|a, b| {
        let (x, y) = (items[a.index].as_ref(), items[b.index].as_ref());
        x.to_lowercase().cmp(&y.to_lowercase()).then_with(|| x.cmp(y))
    });
    out
}

/// Trim, drop empties and case-insensitive duplicates (first spelling wins),
/// and sort by name, case-insensitive.
pub fn dedupe_names(raw: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<String> = raw.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && seen.insert(s.to_lowercase())).collect();
    out.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b)));
    out
}
