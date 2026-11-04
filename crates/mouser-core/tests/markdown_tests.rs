//! Markdown parsing -> rows (docs/markdown.md).

use mouser_core::markdown::{self, BOLD, CODE, ITALIC, Kind, LINK, MUTED, STRIKE};

fn texts(src: &str, cols: usize) -> Vec<String> {
    markdown::layout(src, cols).iter().map(|l| format!("{}{}", " ".repeat(l.start()), l.text())).collect()
}

#[test]
fn headings_paragraphs_and_emphasis() {
    let l = markdown::layout("# Title\n\nSome **bold**, *it*, `code` and ~~gone~~.", 80);
    assert_eq!(l[0].kind, Kind::Heading(1));
    assert_eq!(l[0].text(), "Title");
    assert!(l[1].spans.is_empty(), "blank row between blocks");
    let p = &l[2];
    let f = |t: &str| p.spans.iter().find(|s| s.text == t).map(|s| s.flags);
    assert_eq!(f("bold"), Some(BOLD));
    assert_eq!(f("it"), Some(ITALIC));
    assert_eq!(f("code"), Some(CODE));
    assert_eq!(f("gone"), Some(STRIKE));
    assert_eq!(p.text(), "Some bold, it, code and gone.");
}

#[test]
fn wraps_to_width_and_breaks_long_words() {
    let t = texts("one two three four five six seven", 12);
    assert!(t.iter().all(|r| markdown::width(r) <= 12), "{t:?}");
    assert_eq!(t[0], "one two");
    let t = texts("abcdefghijklmnopqrstuvwxyz", 10);
    assert_eq!(t, ["abcdefghij", "klmnopqrst", "uvwxyz"]);
}

#[test]
fn lists_nest_and_number() {
    let t = texts("- a\n- b long enough to wrap here\n  - c\n\n1. one\n2. two\n- [x] done\n", 20);
    assert_eq!(t[0], "• a");
    assert_eq!(t[1], "• b long enough to");
    assert_eq!(t[2], "  wrap here", "continuation under the text");
    assert_eq!(t[3], "  • c");
    assert!(t.contains(&"1. one".to_string()) && t.contains(&"2. two".to_string()), "{t:?}");
    assert!(t.contains(&"• [x] done".to_string()), "{t:?}");
}

#[test]
fn code_blocks_keep_lines_and_quotes_nest() {
    let l = markdown::layout("```rust\nfn main() {\n    x();\n}\n```\n\n> quoted\n>> deeper", 40);
    let code: Vec<_> = l.iter().filter(|l| l.kind == Kind::Code).map(|l| l.text()).collect();
    assert_eq!(code, ["fn main() {", "    x();", "}"]);
    let q: Vec<_> = l.iter().filter(|l| l.quote > 0 && !l.spans.is_empty()).map(|l| (l.quote, l.text())).collect();
    assert_eq!(q, [(1, "quoted".to_string()), (2, "deeper".to_string())]);
}

#[test]
fn links_images_and_rules() {
    let l = markdown::layout("see [docs](https://x.y/z) ![logo](a.png)\n\n---", 80);
    let link = l[0].spans.iter().find(|s| s.flags & LINK != 0).unwrap();
    assert_eq!((link.text.as_str(), link.link.as_deref()), ("docs", Some("https://x.y/z")));
    assert!(l[0].spans.iter().any(|s| s.flags & MUTED != 0 && s.text.contains("[image: logo]")));
    assert_eq!(l.last().unwrap().kind, Kind::Rule);
}

#[test]
fn tables_align_columns() {
    let t = texts("| a | bb |\n|---|---|\n| ccc | d |", 40);
    assert_eq!(t, ["a   │ bb", "────┼───", "ccc │ d"]);
    let l = markdown::layout("| h |\n|---|\n| v |", 40);
    assert!(l[0].spans.iter().all(|s| s.flags & BOLD != 0 || s.flags & MUTED != 0));
}

#[test]
fn wide_tables_shrink_to_fit() {
    let t = texts("| aaaaaaaaaaaaaaaaaaaa | bbbbbbbbbbbbbbbbbbbb |\n|---|---|\n| x | y |", 20);
    assert!(t.iter().all(|r| markdown::width(r) <= 20), "{t:?}");
    assert!(t[0].contains('…'));
}

#[test]
fn streaming_partial_markdown_is_stable() {
    // A reply arriving piece by piece: earlier rows do not change as more comes in.
    let full = "Intro line.\n\n```sh\nls -la\necho hi\n```\n\nAfter **bold** text.";
    let mut prev: Vec<String> = Vec::new();
    for n in 1..=full.len() {
        if !full.is_char_boundary(n) {
            continue;
        }
        let cur = texts(&full[..n], 40);
        if prev.len() > 2 {
            assert_eq!(cur[0], prev[0], "first row stays at {n}");
        }
        prev = cur;
    }
    // An unclosed fence is code to the end.
    let l = markdown::layout("```\nopen", 40);
    assert_eq!(l.last().unwrap().kind, Kind::Code);
}

#[test]
fn control_characters_are_neutralised() {
    let out = markdown::to_ansi(&markdown::layout("evil \x1b]52;c;AAAA\x07 [x](http://a\x1bb)", 80), 80, true);
    assert!(!out.contains("\x1b]52"), "no smuggled OSC");
    assert!(!out.contains("\x07"));
    assert!(out.contains("\u{FFFD}"));
}

#[test]
fn ansi_styles_and_osc8_links() {
    let out = markdown::to_ansi(&markdown::layout("# H\n\n**b** [l](https://e.com)\n\n```\nc\n```", 20), 20, true);
    assert!(out.contains("\x1b[1;4;35"), "h1 bold underlined");
    assert!(out.contains("\x1b[0;1mb"), "bold");
    assert!(out.contains("\x1b]8;;https://e.com\x1b\\"), "OSC 8 link");
    assert!(out.contains("\x1b[0;48;5;236;38;5;223mc\x1b[0;48;5;236m                   \x1b[0m"), "code row padded");
    let plain = markdown::to_ansi(&markdown::layout("[l](https://e.com)", 20), 20, false);
    assert!(!plain.contains("\x1b]8"));
}

#[test]
fn is_markdown_names() {
    assert!(markdown::is_markdown("README.md") && markdown::is_markdown("a.MARKDOWN"));
    assert!(!markdown::is_markdown("a.txt") && !markdown::is_markdown("md"));
}
