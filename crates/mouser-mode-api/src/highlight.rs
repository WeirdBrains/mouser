//! Syntax highlighting for Markdown code blocks (docs/markdown.md §Code).
//!
//! A small, dependency-free lexer: keywords, types, strings, numbers,
//! comments, function calls, and diff lines for the languages people paste
//! most. Each surface colours the token classes from the theme's palette.
//!
//! State only flows forward (a block comment opened on one line continues
//! on the next), so a line's tokens never depend on lines after it: a block
//! that is still streaming in never recolours the rows above.

/// Token classes (stored in `markdown::Span::hl`); 0 is plain text.
pub const PLAIN: u8 = 0;
pub const KEYWORD: u8 = 1;
pub const TYPE: u8 = 2;
pub const STRING: u8 = 3;
pub const NUMBER: u8 = 4;
pub const COMMENT: u8 = 5;
pub const FUNC: u8 = 6;
/// Diff `+` lines.
pub const ADDED: u8 = 7;
/// Diff `-` lines.
pub const REMOVED: u8 = 8;
/// Diff hunks, TOML sections, preprocessor lines, Markdown marks.
pub const META: u8 = 9;

/// Languages the lexer knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Python,
    JavaScript,
    Shell,
    Json,
    Toml,
    Yaml,
    Go,
    C,
    Html,
    Css,
    Sql,
    Markdown,
    Diff,
}

/// The language for a fence's info string (`rust`, `rs`, `py`, `ts`, …);
/// only the first word counts. Unknown or empty: no highlighting.
pub fn lang(info: &str) -> Option<Lang> {
    let tag = info.split(|c: char| c.is_whitespace() || c == ',' || c == '{').next()?.trim().to_ascii_lowercase();
    Some(match tag.as_str() {
        "rust" | "rs" => Lang::Rust,
        "python" | "py" | "python3" => Lang::Python,
        "javascript" | "js" | "jsx" | "mjs" | "cjs" | "typescript" | "ts" | "tsx" => Lang::JavaScript,
        "sh" | "bash" | "zsh" | "shell" | "console" | "fish" | "ksh" => Lang::Shell,
        "json" | "jsonc" | "json5" => Lang::Json,
        "toml" => Lang::Toml,
        "yaml" | "yml" => Lang::Yaml,
        "go" | "golang" => Lang::Go,
        "c" | "h" | "cpp" | "c++" | "cc" | "cxx" | "hpp" => Lang::C,
        "html" | "htm" | "xml" | "svg" => Lang::Html,
        "css" | "scss" => Lang::Css,
        "sql" | "psql" | "sqlite" | "mysql" => Lang::Sql,
        "md" | "markdown" => Lang::Markdown,
        "diff" | "patch" => Lang::Diff,
        _ => return None,
    })
}

struct Spec {
    keywords: &'static [&'static str],
    types: &'static [&'static str],
    line_comment: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    quotes: &'static str,
    /// Capitalised identifiers are types.
    caps_types: bool,
    case_insensitive: bool,
}

const RUST_KW: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let",
    "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe",
    "use", "where", "while",
];
const RUST_TY: &[&str] =
    &["bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64"];
const PY_KW: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except",
    "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield", "self", "match", "case",
];
const PY_TY: &[&str] = &["int", "float", "str", "bool", "list", "dict", "set", "tuple", "bytes", "object"];
const JS_KW: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "from",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "null",
    "of",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "yield",
    "interface",
    "type",
    "enum",
    "implements",
    "as",
    "readonly",
    "declare",
    "namespace",
];
const JS_TY: &[&str] = &["string", "number", "boolean", "any", "unknown", "never", "object", "bigint", "symbol"];
const SH_KW: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "in", "function", "return", "local", "export",
    "readonly", "set", "unset", "source", "alias", "exit", "cd", "echo", "printf", "eval", "exec", "sudo",
];
const JSON_KW: &[&str] = &["true", "false", "null"];
const TOML_KW: &[&str] = &["true", "false"];
const YAML_KW: &[&str] = &["true", "false", "null", "yes", "no", "on", "off", "~"];
const GO_KW: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
    "true",
    "false",
    "nil",
    "iota",
];
const GO_TY: &[&str] = &[
    "bool",
    "byte",
    "complex64",
    "complex128",
    "error",
    "float32",
    "float64",
    "int",
    "int8",
    "int16",
    "int32",
    "int64",
    "rune",
    "string",
    "uint",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "uintptr",
    "any",
];
const C_KW: &[&str] = &[
    "auto",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "constexpr",
    "continue",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "extern",
    "false",
    "for",
    "goto",
    "if",
    "inline",
    "namespace",
    "new",
    "nullptr",
    "private",
    "protected",
    "public",
    "register",
    "return",
    "sizeof",
    "static",
    "struct",
    "switch",
    "template",
    "this",
    "throw",
    "true",
    "try",
    "typedef",
    "typename",
    "union",
    "using",
    "virtual",
    "volatile",
    "while",
    "NULL",
];
const C_TY: &[&str] = &[
    "void", "char", "short", "int", "long", "float", "double", "signed", "unsigned", "bool", "size_t", "uint8_t", "uint16_t", "uint32_t", "uint64_t",
    "int8_t", "int16_t", "int32_t", "int64_t", "auto", "std", "string", "vector",
];
const CSS_KW: &[&str] = &["important", "inherit", "initial", "none", "auto", "media", "import", "keyframes"];
const SQL_KW: &[&str] = &[
    "select",
    "from",
    "where",
    "and",
    "or",
    "not",
    "insert",
    "into",
    "values",
    "update",
    "set",
    "delete",
    "create",
    "table",
    "drop",
    "alter",
    "index",
    "join",
    "left",
    "right",
    "inner",
    "outer",
    "on",
    "as",
    "group",
    "by",
    "order",
    "having",
    "limit",
    "offset",
    "null",
    "is",
    "in",
    "primary",
    "key",
    "foreign",
    "references",
    "distinct",
    "union",
    "all",
    "case",
    "when",
    "then",
    "else",
    "end",
    "exists",
    "like",
    "asc",
    "desc",
    "begin",
    "commit",
    "rollback",
    "with",
    "default",
    "true",
    "false",
];
const SQL_TY: &[&str] =
    &["int", "integer", "bigint", "text", "varchar", "char", "boolean", "real", "float", "date", "timestamp", "blob", "serial", "numeric"];

fn spec(l: Lang) -> Spec {
    let s = |keywords, types, line_comment, block_comment, quotes, caps_types| Spec {
        keywords,
        types,
        line_comment,
        block_comment,
        quotes,
        caps_types,
        case_insensitive: false,
    };
    match l {
        Lang::Rust => s(RUST_KW, RUST_TY, &["//"], Some(("/*", "*/")), "\"", true),
        Lang::Python => s(PY_KW, PY_TY, &["#"], None, "\"'", true),
        Lang::JavaScript => s(JS_KW, JS_TY, &["//"], Some(("/*", "*/")), "\"'`", true),
        Lang::Shell => s(SH_KW, &[], &["#"], None, "\"'", false),
        Lang::Json => s(JSON_KW, &[], &[], None, "\"", false),
        Lang::Toml => s(TOML_KW, &[], &["#"], None, "\"'", false),
        Lang::Yaml => s(YAML_KW, &[], &["#"], None, "\"'", false),
        Lang::Go => s(GO_KW, GO_TY, &["//"], Some(("/*", "*/")), "\"'`", true),
        Lang::C => s(C_KW, C_TY, &["//"], Some(("/*", "*/")), "\"'", false),
        Lang::Html => s(&[], &[], &[], Some(("<!--", "-->")), "\"'", false),
        Lang::Css => s(CSS_KW, &[], &[], Some(("/*", "*/")), "\"'", false),
        Lang::Sql => Spec { case_insensitive: true, ..s(SQL_KW, SQL_TY, &["--"], Some(("/*", "*/")), "'\"", false) },
        Lang::Markdown | Lang::Diff => s(&[], &[], &[], None, "", false),
    }
}

/// Highlights a code block line by line, carrying open block comments.
pub struct Highlighter {
    lang: Lang,
    spec: Spec,
    in_block: bool,
}

/// `(text, class)` runs of one line; their texts joined are the line.
pub type Runs = Vec<(String, u8)>;

impl Highlighter {
    pub fn new(lang: Lang) -> Self {
        Highlighter { lang, spec: spec(lang), in_block: false }
    }

    /// Runs for the next line of the block.
    pub fn line(&mut self, line: &str) -> Runs {
        let mut out: Runs = Vec::new();
        match self.lang {
            Lang::Diff => {
                let c = if line.starts_with("+++") || line.starts_with("---") || line.starts_with("@@") || line.starts_with("diff ") {
                    META
                } else if line.starts_with('+') {
                    ADDED
                } else if line.starts_with('-') {
                    REMOVED
                } else {
                    PLAIN
                };
                push(&mut out, line, c);
                return out;
            }
            Lang::Markdown => {
                let t = line.trim_start();
                let c = if t.starts_with('#') {
                    KEYWORD
                } else if t.starts_with("```") || t.starts_with('>') {
                    META
                } else {
                    PLAIN
                };
                push(&mut out, line, c);
                return out;
            }
            _ => {}
        }
        let ch: Vec<char> = line.chars().collect();
        let at = |i: usize, s: &str| s.chars().enumerate().all(|(k, c)| ch.get(i + k) == Some(&c));
        let text = |a: usize, b: usize| ch[a..b].iter().collect::<String>();
        let mut i = 0;
        // Shell, Python, TOML, YAML: `#` starts a comment only at a word start.
        let hash_word = matches!(self.lang, Lang::Shell | Lang::Toml | Lang::Yaml | Lang::Python);
        // TOML `[section]` / C `#include`.
        let lead = ch.iter().position(|c| !c.is_whitespace()).unwrap_or(ch.len());
        if !self.in_block && ((self.lang == Lang::Toml && ch.get(lead) == Some(&'[')) || (self.lang == Lang::C && ch.get(lead) == Some(&'#'))) {
            push(&mut out, line, META);
            return out;
        }
        while i < ch.len() {
            if self.in_block {
                let (_, end) = self.spec.block_comment.unwrap_or(("", ""));
                let mut j = i;
                while j < ch.len() && !at(j, end) {
                    j += 1;
                }
                let stop = if j < ch.len() {
                    self.in_block = false;
                    j + end.chars().count()
                } else {
                    j
                };
                push(&mut out, &text(i, stop), COMMENT);
                i = stop;
                continue;
            }
            let c = ch[i];
            if let Some((open, _)) = self.spec.block_comment
                && at(i, open)
            {
                self.in_block = true;
                let n = open.chars().count();
                push(&mut out, &text(i, i + n), COMMENT);
                i += n;
                continue;
            }
            if self.spec.line_comment.iter().any(|p| at(i, p) && (!hash_word || *p != "#" || i == 0 || ch[i - 1].is_whitespace())) {
                push(&mut out, &text(i, ch.len()), COMMENT);
                break;
            }
            if self.spec.quotes.contains(c) && !(self.lang == Lang::Rust && c == '\'') {
                let mut j = i + 1;
                while j < ch.len() && ch[j] != c {
                    j += if ch[j] == '\\' { 2 } else { 1 };
                }
                let stop = (j + 1).min(ch.len());
                // JSON / YAML keys (a string before `:`) are drawn as types.
                let key = matches!(self.lang, Lang::Json | Lang::Yaml) && ch[stop..].iter().find(|c| !c.is_whitespace()) == Some(&':');
                push(&mut out, &text(i, stop), if key { TYPE } else { STRING });
                i = stop;
                continue;
            }
            if self.lang == Lang::Html && c == '<' {
                let mut j = i + 1;
                if ch.get(j) == Some(&'/') {
                    j += 1;
                }
                while j < ch.len() && (ch[j].is_alphanumeric() || ch[j] == '-' || ch[j] == '!') {
                    j += 1;
                }
                push(&mut out, &text(i, j), KEYWORD);
                i = j;
                continue;
            }
            if c.is_ascii_digit() || (c == '.' && ch.get(i + 1).is_some_and(|d| d.is_ascii_digit()) && !prev_word(&ch, i)) {
                if prev_word(&ch, i) {
                    push(&mut out, &c.to_string(), PLAIN);
                    i += 1;
                    continue;
                }
                let mut j = i;
                while j < ch.len() && (ch[j].is_ascii_alphanumeric() || ch[j] == '.' || ch[j] == '_') {
                    j += 1;
                }
                push(&mut out, &text(i, j), NUMBER);
                i = j;
                continue;
            }
            if c.is_alphabetic() || c == '_' || (self.lang == Lang::Shell && c == '$') {
                let mut j = i + 1;
                while j < ch.len() && (ch[j].is_alphanumeric() || ch[j] == '_' || (self.lang == Lang::Css && ch[j] == '-')) {
                    j += 1;
                }
                let w = text(i, j);
                let next = ch[j..].iter().find(|c| !c.is_whitespace()).copied();
                let cls = self.word(&w, next, ch.get(j).copied(), i == lead);
                push(&mut out, &w, cls);
                i = j;
                continue;
            }
            push(&mut out, &c.to_string(), PLAIN);
            i += 1;
        }
        out
    }

    fn word(&self, w: &str, next: Option<char>, right_after: Option<char>, first: bool) -> u8 {
        let s = &self.spec;
        let hit = |list: &[&str]| if s.case_insensitive { list.iter().any(|k| k.eq_ignore_ascii_case(w)) } else { list.contains(&w) };
        if w.starts_with('$') {
            return TYPE;
        }
        match self.lang {
            Lang::Toml | Lang::Yaml if first && matches!(next, Some('=') | Some(':')) => return TYPE,
            Lang::Css if next == Some(':') => return TYPE,
            Lang::Html if next == Some('=') => return TYPE,
            Lang::Rust if right_after == Some('!') => return FUNC,
            _ => {}
        }
        if hit(s.keywords) {
            KEYWORD
        } else if hit(s.types) || (s.caps_types && w.chars().next().is_some_and(char::is_uppercase)) {
            TYPE
        } else if (next == Some('(') && !matches!(self.lang, Lang::Json | Lang::Toml | Lang::Yaml | Lang::Html))
            || (self.lang == Lang::Shell && first)
        {
            FUNC
        } else {
            PLAIN
        }
    }
}

fn prev_word(ch: &[char], i: usize) -> bool {
    i > 0 && (ch[i - 1].is_alphanumeric() || ch[i - 1] == '_')
}

fn push(out: &mut Runs, t: &str, c: u8) {
    if t.is_empty() {
        return;
    }
    match out.last_mut() {
        Some((s, k)) if *k == c => s.push_str(t),
        _ => out.push((t.to_string(), c)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_map_to_languages() {
        for (tag, l) in [
            ("rust", Lang::Rust),
            ("rs", Lang::Rust),
            ("py", Lang::Python),
            ("python", Lang::Python),
            ("js", Lang::JavaScript),
            ("ts", Lang::JavaScript),
            ("tsx", Lang::JavaScript),
            ("sh", Lang::Shell),
            ("bash", Lang::Shell),
            ("zsh", Lang::Shell),
            ("json", Lang::Json),
            ("toml", Lang::Toml),
            ("yaml", Lang::Yaml),
            ("yml", Lang::Yaml),
            ("go", Lang::Go),
            ("c", Lang::C),
            ("cpp", Lang::C),
            ("c++", Lang::C),
            ("html", Lang::Html),
            ("css", Lang::Css),
            ("sql", Lang::Sql),
            ("md", Lang::Markdown),
            ("diff", Lang::Diff),
            ("Rust", Lang::Rust),
            ("rust,ignore", Lang::Rust),
            ("py title=x.py", Lang::Python),
        ] {
            assert_eq!(lang(tag), Some(l), "{tag}");
        }
        for tag in ["", "text", "plain", "brainfuck", "  "] {
            assert_eq!(lang(tag), None, "{tag:?}");
        }
    }

    #[test]
    fn runs_keep_the_text_identical() {
        let samples: &[(&str, &[&str])] = &[
            ("rust", &["fn main() { let x: u32 = 0x1F; println!(\"hi {x}\"); } // done", "/* open", "still */ let s = 'c';"]),
            ("py", &["def f(a, b=2):  # note", "    return \"x\" + 'y'"]),
            ("ts", &["const a: string = `t${b}`; /* c */ fn(1.5e3)"]),
            ("sh", &["echo \"$HOME\" | grep -v x # comment", "for f in *.md; do mouser md \"$f\"; done"]),
            ("json", &["{\"a\": [1, 2.5, true, null], \"b\": \"c\"}"]),
            ("toml", &["[package]", "name = \"mouser\" # x"]),
            ("yaml", &["key: value  # c", "- 'item'"]),
            ("go", &["func main() { fmt.Println(\"hi\") }"]),
            ("cpp", &["#include <stdio.h>", "int main(void) { return 0; }"]),
            ("html", &["<a href=\"x\">link</a> <!-- c -->"]),
            ("css", &["a { color: #fff; } /* c */"]),
            ("sql", &["SELECT * FROM t WHERE id = 1; -- c"]),
            ("md", &["# Title", "text"]),
            ("diff", &["@@ -1 +1 @@", "-old", "+new", " same"]),
            ("rust", &["", "\u{1F600} wide 世界 \"unterminated"]),
        ];
        for (tag, lines) in samples {
            let mut h = Highlighter::new(lang(tag).unwrap());
            for l in *lines {
                let runs = h.line(l);
                let joined: String = runs.iter().map(|r| r.0.as_str()).collect();
                assert_eq!(&joined, l, "{tag}");
            }
        }
    }

    #[test]
    fn classes() {
        let mut h = Highlighter::new(Lang::Rust);
        let r = h.line("fn main() { let s = \"x\"; } // c");
        let cls = |t: &str| r.iter().find(|x| x.0.contains(t)).map(|x| x.1);
        assert_eq!(cls("fn"), Some(KEYWORD));
        assert_eq!(cls("main"), Some(FUNC));
        assert_eq!(cls("\"x\""), Some(STRING));
        assert_eq!(cls("// c"), Some(COMMENT));
        assert_eq!(Highlighter::new(Lang::Diff).line("+a"), vec![("+a".to_string(), ADDED)]);
        let mut h = Highlighter::new(Lang::C);
        assert_eq!(h.line("/* a").last().unwrap().1, COMMENT);
        assert_eq!(h.line("b */ x")[0], ("b */".to_string(), COMMENT));
    }
}
