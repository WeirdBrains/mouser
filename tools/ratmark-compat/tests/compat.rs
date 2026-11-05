//! Checks on measured behaviour. `fix_*` hold for the WeirdBrains fix branch;
//! `gap_*` record a difference from Mouser that is still open, so a change
//! on either side shows up here.

use ratmark_compat::*;

fn grapheme_docs() -> Vec<String> {
    cases().into_iter().filter(|(n, _)| n.starts_with("case:graphemes")).map(|(_, s)| s).collect()
}

#[test]
fn fix_keeps_grapheme_clusters_whole_when_wrapping() {
    for src in grapheme_docs() {
        for w in [1, 2, 3, 4, 5, 8, 24] {
            let r = rows(Who::Fix, &src, w);
            assert_eq!(grapheme_splits(&r), 0, "width {w}: {r:?}");
        }
    }
}

#[test]
fn baseline_splits_grapheme_clusters() {
    // Why the fix exists: upstream 6483e72 cuts inside a cluster.
    let n: usize = grapheme_docs().iter().flat_map(|s| [2, 3, 5].map(|w| grapheme_splits(&rows(Who::Base, s, w)))).sum();
    assert!(n > 0, "upstream no longer splits clusters; drop the fix");
}

#[test]
fn fix_rows_fit_by_ratatuis_width() {
    for d in corpus() {
        for w in [24, 40, 80] {
            let r = rows(Who::Fix, &d.src, w);
            assert_eq!(
                over_unicode(&r, w),
                0,
                "{} at {w}: {:?}",
                d.name,
                r.iter().filter(|x| unicode_width::UnicodeWidthStr::width(x.as_str()) > w).collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn fix_matches_baseline_without_graphemes() {
    // The fix changes nothing for text without multi-code-point clusters.
    for d in corpus().iter().filter(|d| !d.name.starts_with("case:graphemes")) {
        for w in [24, 40, 80] {
            assert_eq!(rows(Who::Fix, &d.src, w), rows(Who::Base, &d.src, w), "{} at {w}", d.name);
        }
    }
}

#[test]
fn both_keep_the_first_row_while_streaming_like_mousers_test() {
    // Mouser's own check (markdown_tests.rs streaming_partial_markdown_is_stable).
    let full = cases().into_iter().find(|(n, _)| *n == "case:streaming").unwrap().1;
    for who in Who::ALL {
        let mut prev: Vec<String> = Vec::new();
        for n in (1..=full.len()).filter(|&n| full.is_char_boundary(n)) {
            let cur = rows(who, &full[..n], 40);
            if prev.len() > 2 {
                assert_eq!(cur[0], prev[0], "{}: first row at {n}", who.name());
            }
            prev = cur;
        }
    }
}

#[test]
fn gap_control_characters_reach_ratmarks_rows() {
    let src = cases().into_iter().find(|(n, _)| *n == "case:control").unwrap().1;
    assert_eq!(control_rows(&rows(Who::Mouser, &src, 80)), 0);
    assert!(control_rows(&rows(Who::Fix, &src, 80)) > 0, "ratmark now neutralises control characters");
}

#[test]
fn gap_grid_width_differs_for_emoji_sequences() {
    // Mouser's grid counts a ZWJ family as 6 cells (2 per person), Ratatui as 2.
    assert_eq!(mouser_mode_api::markdown::width(FAMILY), 6);
    assert_eq!(unicode_width::UnicodeWidthStr::width(FAMILY), 2);
}
