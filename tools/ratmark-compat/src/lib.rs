//! Mouser's Markdown layout next to ratmark's, on the same documents and
//! widths. Every number in the report is measured here; nothing is assumed
//! about either renderer.

use mouser_mode_api::markdown;
use std::path::Path;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// A document to lay out.
pub struct Doc {
    pub name: String,
    pub src: String,
}

/// Small cases that each exercise one thing, then Mouser's own docs.
pub fn corpus() -> Vec<Doc> {
    let mut v: Vec<Doc> = cases().into_iter().map(|(n, s)| Doc { name: n.into(), src: s.into() }).collect();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = vec![root.join("README.md")];
    if let Ok(rd) = std::fs::read_dir(root.join("docs")) {
        let mut d: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "md")).collect();
        d.sort();
        files.extend(d);
    }
    for f in files {
        if let Ok(src) = std::fs::read_to_string(&f) {
            let name = f.strip_prefix(&root).unwrap_or(&f).display().to_string();
            v.push(Doc { name, src });
        }
    }
    v
}

pub const FAMILY: &str = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
pub const FLAG: &str = "\u{1f1ef}\u{1f1f5}";
pub const HEART: &str = "\u{2764}\u{fe0f}";
pub const E_ACUTE: &str = "e\u{301}";

/// The cases from Mouser's markdown tests (crates/mouser-core/tests/markdown_tests.rs)
/// plus grapheme and control-character cases.
pub fn cases() -> Vec<(&'static str, String)> {
    vec![
        ("case:headings", "# Title\n\nSome *em* and **strong** text.\n\n## Sub\n\nMore.".into()),
        ("case:wrap", "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda supercalifragilisticexpialidocious".into()),
        ("case:lists", "- one\n- two\n  - nested\n1. first\n2. second\n- [x] done\n- [ ] todo".into()),
        ("case:code-quote", "```rust\nfn main() {}\n    indented\n```\n\n> quote\n>> deeper".into()),
        ("case:links", "See [the docs](https://example.com) and ![alt](img.png)\n\n---\n\nend".into()),
        ("case:table", "| a | b |\n|---|--:|\n| 1 | 22 |\n| long cell text | x |".into()),
        ("case:wide-table", "| a | b | c |\n|---|---|---|\n| aaaaaaaaaaaaaaaaaaaa | bbbbbbbbbbbbbbbbbbbb | cccccccccccccccccccc |".into()),
        ("case:streaming", "Intro line.\n\n```sh\nls -la\necho hi\n```\n\nAfter **bold** text.".into()),
        (
            "case:graphemes",
            format!("{FAMILY}{FLAG}{HEART}{E_ACUTE}{E_ACUTE}{FAMILY}{FAMILY}{FLAG}{FLAG} and {FAMILY} family {E_ACUTE}{E_ACUTE}{E_ACUTE}{E_ACUTE}"),
        ),
        ("case:graphemes-code", format!("```\n{FAMILY}{FAMILY}{FAMILY}{FLAG}{FLAG}{FLAG}{E_ACUTE}{E_ACUTE}{E_ACUTE}{E_ACUTE}\n```")),
        ("case:cjk", "日本語のテキストを折り返す日本語のテキストを折り返す".into()),
        ("case:control", "evil \x1b]52;c;AAAA\x07 text and a [link](http://a\x1bb)".into()),
    ]
}

/// Which renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    Mouser,
    /// ratmark at the upstream baseline.
    Base,
    /// ratmark with the WeirdBrains fix branch.
    Fix,
}

impl Who {
    pub const ALL: [Who; 3] = [Who::Mouser, Who::Base, Who::Fix];
    pub fn name(self) -> &'static str {
        match self {
            Who::Mouser => "mouser",
            Who::Base => "ratmark 6483e72",
            Who::Fix => "ratmark fix",
        }
    }
}

/// The document's rows as plain text, trailing spaces trimmed.
pub fn rows(who: Who, src: &str, width: usize) -> Vec<String> {
    let v: Vec<String> = match who {
        Who::Mouser => markdown::to_text(&markdown::layout(src, width), width).lines().map(String::from).collect(),
        Who::Base => ratmark_base::Rendered::render(src, width as u16, &ratmark_base::Theme::plain()).lines.iter().map(|l| l.to_string()).collect(),
        Who::Fix => ratmark::Rendered::render(src, width as u16, &ratmark::Theme::plain()).lines.iter().map(|l| l.to_string()).collect(),
    };
    v.into_iter().map(|s| s.trim_end().to_string()).collect()
}

/// Rows wider than `width` by Mouser's grid rule (`markdown::width`: joiners
/// and variation selectors 0, everything else per char).
pub fn over_grid(rows: &[String], width: usize) -> usize {
    rows.iter().filter(|r| markdown::width(r) > width).count()
}

/// Rows wider than `width` by unicode-width's string rule, which is what
/// Ratatui's buffer uses (one width per grapheme cluster).
pub fn over_unicode(rows: &[String], width: usize) -> usize {
    rows.iter().filter(|r| r.width() > width).count()
}

/// Row breaks that land inside a grapheme cluster: the last cluster of a row
/// and the first of the next join into one.
pub fn grapheme_splits(rows: &[String]) -> usize {
    rows.windows(2)
        .filter(|w| {
            let (Some(a), Some(b)) = (w[0].graphemes(true).next_back(), w[1].graphemes(true).next()) else { return false };
            format!("{a}{b}").graphemes(true).count() == 1
        })
        .count()
}

/// Rows holding a control character (an escape sequence could reach the terminal).
pub fn control_rows(rows: &[String]) -> usize {
    rows.iter().filter(|r| r.chars().any(char::is_control)).count()
}

/// Streaming: the document rendered at growing prefixes. For each step,
/// `back` is how many rows of the shorter prefix's layout changed or went
/// away in the next one (counted from the end). The row being written is 1;
/// more means earlier rows moved.
#[derive(Debug, Default, Clone)]
pub struct Streaming {
    pub steps: usize,
    pub max_back: usize,
    /// Steps where more than the last row moved.
    pub over1: usize,
    /// Steps where more than the last 3 rows moved.
    pub over3: usize,
    /// Where `max_back` happened: the byte offset and the prefix's tail.
    pub worst: Option<(usize, String)>,
}

pub fn streaming(who: Who, src: &str, width: usize, max_steps: usize) -> Streaming {
    let bounds: Vec<usize> = (1..=src.len()).filter(|&n| src.is_char_boundary(n)).collect();
    let stride = bounds.len().div_ceil(max_steps.max(1)).max(1);
    let picks: Vec<usize> = bounds.iter().copied().step_by(stride).chain(std::iter::once(src.len())).collect();
    let mut s = Streaming::default();
    let mut prev: Option<Vec<String>> = None;
    for &n in &picks {
        let cur = rows(who, &src[..n], width);
        if let Some(p) = &prev {
            let first = p.iter().zip(&cur).take_while(|(a, b)| a == b).count();
            let back = p.len() - first;
            s.steps += 1;
            if back > 1 {
                s.over1 += 1;
            }
            if back > 3 {
                s.over3 += 1;
            }
            if back > s.max_back {
                s.max_back = back;
                let tail: String = src[..n].chars().rev().take(24).collect::<Vec<_>>().into_iter().rev().collect();
                s.worst = Some((n, tail));
            }
        }
        prev = Some(cur);
    }
    s
}

/// The first place two word lists differ, with a few words of context.
pub fn first_word_diff(a: &[String], b: &[String]) -> Option<(String, String)> {
    let i = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    if i == a.len() && i == b.len() {
        return None;
    }
    let ctx = |v: &[String]| v[i.saturating_sub(2)..(i + 4).min(v.len())].join(" ");
    Some((ctx(a), ctx(b)))
}

/// Rows equal to Mouser's, in order (a rough measure of how close the look is).
pub fn same_rows(a: &[String], b: &[String]) -> usize {
    a.iter().zip(b).filter(|(x, y)| x == y).count()
}

/// The words of the output, so renderers that draw marks differently can
/// still be compared on content.
pub fn words(rows: &[String]) -> Vec<String> {
    rows.iter()
        .flat_map(|r| r.split_whitespace())
        .map(|w| w.trim_matches(|c: char| "│─•‹›…|┃┆▎┼".contains(c)).to_string())
        .filter(|w| !w.is_empty())
        .collect()
}
