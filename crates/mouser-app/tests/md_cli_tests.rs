//! `mouser md`: Markdown rendered into the terminal (docs/markdown.md).

use mouser_app::md_cli::{Color, render, run};

const DOC: &str = "# Title\n\nSome **bold** and `code`, a [link](https://example.com).\n\n```sh\nls -la\n```\n\n- one\n- two\n\n> quoted\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";

#[test]
fn styled_output_has_sgr_and_osc8_links_and_no_raw_markup() {
    let s = render(DOC, 60, true);
    assert!(s.contains("\x1b["), "SGR styles");
    assert!(s.contains("\x1b]8;;https://example.com\x1b\\"), "OSC 8 link");
    let t = strip(&s);
    assert!(t.contains("Title") && t.contains("ls -la") && t.contains("quoted"));
    assert!(!s.contains("**") && !s.contains("```"), "no raw markup");
}

#[test]
fn plain_output_has_no_escapes_and_fits_the_width() {
    let s = render(DOC, 40, false);
    assert!(!s.contains('\x1b'), "{s:?}");
    assert!(s.contains("Title") && s.contains("bold") && s.contains("one"));
    for l in s.lines() {
        assert!(l.chars().count() <= 40, "{l:?} wider than 40");
    }
}

#[test]
fn files_render_one_after_another() {
    let d = tempfile::tempdir().unwrap();
    let (a, b) = (d.path().join("a.md"), d.path().join("b.md"));
    std::fs::write(&a, "# A\n").unwrap();
    std::fs::write(&b, "# B\n").unwrap();
    let mut out = Vec::new();
    let files = [a.display().to_string(), b.display().to_string()];
    run(&files, Some(50), Color::Never, &mut out).unwrap();
    let s = String::from_utf8(out).unwrap();
    assert!(s.find('A').unwrap() < s.find('B').unwrap(), "{s}");
    assert!(!s.contains('\x1b'));
    assert!(run(&["/nonexistent/x.md".into()], None, Color::Never, &mut Vec::new()).is_err());
}

#[test]
fn control_characters_never_pass_through() {
    let s = render("hi \x1b]0;pwn\x07 there [x](https://e.com/\x1b[2J)\n", 60, true);
    assert!(!s.contains("\x1b]0;") && !s.contains("\x1b[2J"), "{s:?}");
}

/// SGR sequences removed (OSC 8 links too).
fn strip(s: &str) -> String {
    let (mut out, mut it) = (String::new(), s.chars().peekable());
    while let Some(c) = it.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('[') => while it.next().is_some_and(|c| !c.is_ascii_alphabetic()) {},
            Some(']') => {
                while let Some(c) = it.next() {
                    if c == '\x1b' {
                        it.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[test]
fn code_blocks_are_highlighted_and_keep_their_text() {
    let src = "```rust\nfn main() { let s = \"hi\"; } // c\n```\n\n```text\nplain words\n```\n";
    let styled = render(src, 60, true);
    assert!(styled.contains("\x1b[0;48;5;236;35mfn"), "keyword in the palette's magenta: {styled:?}");
    assert!(styled.contains(";32m\"hi\""), "string in green");
    assert!(styled.contains(";3;90m// c"), "comment muted");
    assert!(!styled.contains(";35mplain"), "unknown language stays plain");
    // Same text, same rows as the unstyled output, apart from padding.
    let a: Vec<String> = strip(&styled).lines().map(|l| l.trim_end().to_string()).collect();
    let b: Vec<String> = render(src, 60, false).lines().map(|l| l.trim_end().to_string()).collect();
    assert_eq!(a, b);
    assert!(b.iter().any(|l| l == "fn main() { let s = \"hi\"; } // c"));
}
