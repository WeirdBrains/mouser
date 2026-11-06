//! In-app help: the docs/ guides, embedded at build time (build.rs), shown by
//! `mouser help [TOPIC | search WORDS]`. See docs/help.md.

include!(concat!(env!("OUT_DIR"), "/help_topics.rs"));

/// The first `# ` heading of a topic (its title).
pub fn title(md: &str) -> &str {
    md.lines().find_map(|l| l.strip_prefix("# ")).unwrap_or("").trim()
}

/// A topic by id (`models`, `protocol`, `index`), case-insensitive; a
/// trailing `.md` or a `docs/` prefix is ignored.
pub fn topic(id: &str) -> Option<(&'static str, &'static str)> {
    let id = id.trim().trim_start_matches("docs/").trim_end_matches(".md").to_ascii_lowercase();
    let id = if id.is_empty() || id == "readme" { "index".to_string() } else { id };
    TOPICS.iter().copied().find(|(t, _)| t.to_ascii_lowercase() == id)
}

/// Topics whose text contains every word (case-insensitive), with the first
/// matching line of each. Best matches (words in the title) first.
pub fn search(words: &[String]) -> Vec<(&'static str, String)> {
    let words: Vec<String> = words.iter().map(|w| w.to_lowercase()).filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return vec![];
    }
    let mut hits: Vec<(bool, &'static str, String)> = TOPICS
        .iter()
        .filter(|(_, md)| {
            let l = md.to_lowercase();
            words.iter().all(|w| l.contains(w.as_str()))
        })
        .map(|(id, md)| {
            let line = md.lines().find(|l| words.iter().any(|w| l.to_lowercase().contains(w.as_str()))).unwrap_or("").trim().to_string();
            let in_title = words.iter().any(|w| title(md).to_lowercase().contains(w.as_str()));
            (!in_title, *id, line)
        })
        .collect();
    hits.sort();
    hits.into_iter().map(|(_, id, line)| (id, line)).collect()
}

/// `mouser help` with no topic: every topic and its title.
pub fn list() -> String {
    let w = TOPICS.iter().map(|(id, _)| id.len()).max().unwrap_or(0);
    let mut s = String::from("Mouser help topics (`mouser help TOPIC`, `mouser help search WORDS`):\n\n");
    for (id, md) in TOPICS {
        s.push_str(&format!("  {id:<w$}  {}\n", title(md)));
    }
    s
}

/// What `mouser help ARGS` prints, or an error message.
pub fn run(args: &[String]) -> Result<String, String> {
    match args {
        [] => Ok(list()),
        [s, rest @ ..] if s == "search" => {
            let hits = search(rest);
            if hits.is_empty() {
                return Err(format!("no help topic mentions {:?}", rest.join(" ")));
            }
            Ok(hits.iter().map(|(id, line)| format!("{id}: {line}\n")).collect())
        }
        [id] => topic(id).map(|(_, md)| md.to_string()).ok_or_else(|| {
            let hint = search(std::slice::from_ref(id)).into_iter().take(5).map(|(i, _)| i).collect::<Vec<_>>().join(", ");
            if hint.is_empty() { format!("no help topic {id:?}; `mouser help` lists them") } else { format!("no help topic {id:?}; try: {hint}") }
        }),
        _ => Err("usage: mouser help [TOPIC | search WORDS]".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_guide_is_a_topic_with_a_title() {
        for id in ["index", "models", "mcp", "modes", "windows", "engines", "help", "cli", "protocol"] {
            let (_, md) = topic(id).unwrap_or_else(|| panic!("{id}"));
            assert!(!title(md).is_empty(), "{id}");
        }
        assert!(topic("docs/models.md").is_some() && topic("MODELS").is_some());
        assert!(!TOPICS.iter().any(|(id, _)| { id.contains("historical") || *id == "HANDOVER" || *id == "deploy-site" || *id == "release" }));
    }

    #[test]
    fn the_index_links_every_topic() {
        let (_, index) = topic("index").unwrap();
        for (id, _) in TOPICS.iter().filter(|(id, _)| *id != "index") {
            let link = if *id == "protocol" { "./protocol/README.md".to_string() } else { format!("./{id}.md") };
            assert!(index.contains(&link), "docs/README.md does not link {link}");
        }
    }

    #[test]
    fn search_and_run() {
        let hits = search(&["trackpad".into()]);
        assert!(hits.iter().any(|(id, _)| *id == "terminal"), "{hits:?}");
        assert!(run(&[]).unwrap().contains("models"));
        assert!(run(&["models".into()]).unwrap().contains("grok"));
        assert!(run(&["nope-nothing".into()]).is_err());
        assert!(run(&["search".into(), "default".into(), "terminal".into()]).unwrap().contains("windows"));
    }
}
