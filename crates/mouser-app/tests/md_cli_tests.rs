//! `mouser md`: Markdown rendered into the terminal (docs/markdown.md).

use mouser_app::md_cli::{Color, render, run};

const DOC: &str = "# Title\n\nSome **bold** and `code`, a [link](https://example.com).\n\n```sh\nls -la\n```\n\n- one\n- two\n\n> quoted\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";

#[test]
fn styled_output_has_sgr_and_osc8_links_and_no_raw_markup() {
    let s = render(DOC, 60, true);
    assert!(s.contains("\x1b["), "SGR styles");
    assert!(s.contains("\x1b]8;;https://example.com\x1b\\"), "OSC 8 link");
    assert!(s.contains("Title") && s.contains("ls -la") && s.contains("quoted"));
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
