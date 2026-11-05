//! `cargo run --release` prints the comparison report (Markdown) to stdout.

use ratmark_compat::*;

const WIDTHS: [usize; 3] = [24, 40, 80];

fn main() {
    let docs = corpus();
    println!("# ratmark compatibility report\n");
    println!("{} documents (cases plus Mouser's README and docs), widths {:?}.\n", docs.len(), WIDTHS);
    println!("## Totals\n");
    println!(
        "| renderer | rows | rows equal to Mouser's | docs with Mouser's words | over width (grid) | over width (unicode) | grapheme splits | rows with control chars |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    for who in Who::ALL {
        let (mut rows_n, mut same, mut words_eq, mut og, mut ou, mut gs, mut cr, mut runs) = (0, 0, 0, 0, 0, 0, 0, 0);
        for d in &docs {
            for &w in &WIDTHS {
                let m = rows(Who::Mouser, &d.src, w);
                let r = rows(who, &d.src, w);
                runs += 1;
                rows_n += r.len();
                same += same_rows(&m, &r);
                words_eq += usize::from(words(&m) == words(&r));
                og += over_grid(&r, w);
                ou += over_unicode(&r, w);
                gs += grapheme_splits(&r);
                cr += control_rows(&r);
            }
        }
        println!("| {} | {rows_n} | {same} | {words_eq}/{runs} | {og} | {ou} | {gs} | {cr} |", who.name());
    }
    println!("\n## Per case (width 24)\n");
    println!("| case | renderer | rows | grid over | unicode over | splits | control |");
    println!("|---|---|---|---|---|---|---|");
    for d in docs.iter().filter(|d| d.name.starts_with("case:")) {
        for who in Who::ALL {
            let r = rows(who, &d.src, 24);
            println!(
                "| {} | {} | {} | {} | {} | {} | {} |",
                d.name,
                who.name(),
                r.len(),
                over_grid(&r, 24),
                over_unicode(&r, 24),
                grapheme_splits(&r),
                control_rows(&r)
            );
        }
    }
    println!("\n## Streaming (width 40, at most 400 prefixes per document)\n");
    println!("`back`: rows of one prefix's layout that changed or went away in the next, counted from the end. 1 is the row being written.\n");
    println!("| renderer | steps | max back | steps with back > 1 | steps with back > 3 | worst at |");
    println!("|---|---|---|---|---|---|");
    for who in Who::ALL {
        let mut t = Streaming::default();
        let mut worst_doc = String::new();
        for d in &docs {
            let s = streaming(who, &d.src, 40, 400);
            t.steps += s.steps;
            t.over1 += s.over1;
            t.over3 += s.over3;
            if s.max_back > t.max_back {
                t.max_back = s.max_back;
                if let Some((n, tail)) = &s.worst {
                    worst_doc = format!("{} @{n} `…{}`", d.name, tail.replace('\n', "⏎").replace('|', "\\|").replace('`', "'"));
                }
            }
        }
        println!("| {} | {} | {} | {} | {} | {} |", who.name(), t.steps, t.max_back, t.over1, t.over3, worst_doc);
    }
    println!("\n## Worst streaming documents (ratmark fix, width 40)\n");
    let mut worst: Vec<(usize, usize, String)> = docs
        .iter()
        .map(|d| {
            let s = streaming(Who::Fix, &d.src, 40, 400);
            let m = streaming(Who::Mouser, &d.src, 40, 400);
            let at = s.worst.map(|(n, t)| format!("@{n} `…{}`", t.replace('\n', "⏎").replace('|', "\\|").replace('`', "'"))).unwrap_or_default();
            (s.max_back, m.max_back, format!("{} {at}", d.name))
        })
        .collect();
    worst.sort_by_key(|w| std::cmp::Reverse(w.0));
    println!("| ratmark max back | mouser max back | document, where |");
    println!("|---|---|---|");
    for (r, m, n) in worst.iter().take(8) {
        println!("| {r} | {m} | {n} |");
    }
    println!("\n## First content difference per document (ratmark fix vs Mouser, width 80)\n");
    println!("| document | mouser | ratmark |");
    println!("|---|---|---|");
    for d in &docs {
        let (m, r) = (words(&rows(Who::Mouser, &d.src, 80)), words(&rows(Who::Fix, &d.src, 80)));
        if let Some((a, b)) = first_word_diff(&m, &r) {
            let esc = |s: String| s.replace('|', "\\|").replace('\x1b', "␛").replace('\x07', "␇");
            println!("| {} | {} | {} |", d.name, esc(a), esc(b));
        }
    }
    println!("\n## Samples (width 24)\n");
    for name in ["case:graphemes", "case:wide-table", "case:code-quote", "case:control"] {
        let d = docs.iter().find(|d| d.name == name).unwrap();
        println!("### {name}\n");
        for who in Who::ALL {
            println!("{}:\n```", who.name());
            for r in rows(who, &d.src, 24) {
                println!("{}", r.replace('\x1b', "␛").replace('\x07', "␇"));
            }
            println!("```");
        }
    }
}
