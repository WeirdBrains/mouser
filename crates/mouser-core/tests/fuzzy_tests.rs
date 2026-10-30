use mouser_core::fuzzy::{dedupe_names, filter_sorted, fuzzy_match};

#[test]
fn subsequence_case_insensitive() {
    assert!(fuzzy_match("jbm", "JetBrains Mono").is_some());
    assert!(fuzzy_match("JETMONO", "JetBrains Mono").is_some());
    assert!(fuzzy_match("mj", "JetBrains Mono").is_none());
    assert!(fuzzy_match("xyz", "Fira Code").is_none());
    assert_eq!(fuzzy_match("", "Anything"), Some((0, vec![])));
}

#[test]
fn positions_prefer_word_starts() {
    let (_, pos) = fuzzy_match("jbm", "JetBrains Mono").unwrap();
    assert_eq!(pos, vec![0, 3, 10]);
    let (_, pos) = fuzzy_match("fc", "Fira Code").unwrap();
    assert_eq!(pos, vec![0, 5]);
}

#[test]
fn scores_reward_word_starts_and_runs() {
    let (prefix, _) = fuzzy_match("mono", "Monoid").unwrap();
    let (later, _) = fuzzy_match("mono", "Symbols Nerd Font Mono").unwrap();
    assert!(prefix > later);
    // Contiguous beats scattered.
    let (a, _) = fuzzy_match("code", "Fira Code").unwrap();
    let (b, _) = fuzzy_match("code", "Cascadia Mono Device").unwrap();
    assert!(a > b);
}

fn names_for<'a>(q: &str, names: &[&'a str]) -> Vec<&'a str> {
    filter_sorted(q, names).iter().map(|m| names[m.index]).collect()
}

#[test]
fn filtered_results_stay_alphabetical() {
    let names = ["Noto Sans Mono", "monoid", "Hack", "Ubuntu Mono", "Symbols Nerd Font Mono", "DejaVu Sans Mono"];
    // The best match ("monoid") does not jump ahead: name order only.
    assert_eq!(names_for("mono", &names), ["DejaVu Sans Mono", "monoid", "Noto Sans Mono", "Symbols Nerd Font Mono", "Ubuntu Mono"]);
    assert_eq!(names_for("", &names), ["DejaVu Sans Mono", "Hack", "monoid", "Noto Sans Mono", "Symbols Nerd Font Mono", "Ubuntu Mono"]);
    let names = ["Source Code Pro", "Fira Sans Condensed", "Fira Code"];
    assert_eq!(names_for("fico", &names), ["Fira Code", "Fira Sans Condensed"]);
    assert!(names_for("zzz", &names).is_empty());
    assert_eq!(filter_sorted("fc", &["Fira Code"])[0].positions, vec![0, 5]);
}

#[test]
fn dedupe_is_case_insensitive_trimmed_and_sorted() {
    let raw = ["Fira Code", "fira code ", "", "  ", "DejaVu Sans", "Fira Code", "apple"].map(String::from);
    assert_eq!(dedupe_names(raw), vec!["apple", "DejaVu Sans", "Fira Code"]);
}
