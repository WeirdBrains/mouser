//! Markdown layout (docs/markdown.md): CommonMark plus tables, strikethrough
//! and task lists, parsed by pulldown-cmark into styled rows of terminal
//! cells wrapped to a width. Every surface draws the same rows its own way:
//! commander previews and the chat transcript as panel items, `mouser md`
//! as SGR / OSC 8 text in the terminal ([`to_ansi`]).
//!
//! Text is sanitised (control characters become `�`), so a file can never
//! smuggle escape sequences through `mouser md`.

use crate::highlight::{self, Highlighter};
use unicode_width::UnicodeWidthChar;

/// Terminal cells for `c` (the same rule as Mouser's grid: joiners and
/// variation selectors 0, regional indicators and wide chars 2).
pub fn char_width(c: char) -> usize {
    if c == '\u{200d}' || c == '\u{fe0f}' || c == '\u{fe0e}' {
        return 0;
    }
    if ('\u{1f1e6}'..='\u{1f1ff}').contains(&c) {
        return 2;
    }
    c.width().unwrap_or(0).min(2)
}
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// Span flags.
pub const BOLD: u8 = 1;
pub const ITALIC: u8 = 2;
/// Inline code (or a code block's text).
pub const CODE: u8 = 4;
pub const LINK: u8 = 8;
pub const STRIKE: u8 = 16;
/// List marks, table rules, image placeholders.
pub const MUTED: u8 = 32;

/// A run of text in one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub flags: u8,
    /// Target of a link span.
    pub link: Option<String>,
    /// Code block token class (`highlight::KEYWORD`, …; 0 plain).
    pub hl: u8,
}

impl Span {
    fn new(text: impl Into<String>, flags: u8) -> Self {
        Span { text: text.into(), flags, link: None, hl: 0 }
    }
    pub fn width(&self) -> usize {
        width(&self.text)
    }
}

/// What a row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    /// Heading level 1–6.
    Heading(u8),
    /// A row of a fenced or indented code block.
    Code,
    /// A thematic break: a rule across the width.
    Rule,
    /// A table row (cells already padded and separated).
    Table,
}

/// One laid-out row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: Kind,
    /// Block quote depth: a bar per level, two cells each, before `indent`.
    pub quote: u8,
    /// Blank cells after the quote bars (list nesting).
    pub indent: u16,
    pub spans: Vec<Span>,
}

impl Line {
    fn new(kind: Kind, quote: u8, indent: usize) -> Self {
        Line { kind, quote, indent: indent as u16, spans: Vec::new() }
    }
    /// The row's text without bars or indent.
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
    /// Cell column where the spans start.
    pub fn start(&self) -> usize {
        self.quote as usize * 2 + self.indent as usize
    }
    /// Cells used, bars and indent included.
    pub fn width(&self) -> usize {
        self.start() + self.spans.iter().map(Span::width).sum::<usize>()
    }
}

/// Display width in cells.
pub fn width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

fn clean(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c == '\t' {
                ' '
            } else if c.is_control() {
                '\u{FFFD}'
            } else {
                c
            }
        })
        .collect()
}

/// True for `.md` / `.markdown` names.
pub fn is_markdown(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "md" | "markdown" | "mdown" | "mkd")
}

struct List {
    next: Option<u64>,
    /// Indent of this list's items (cells, before the mark).
    base: usize,
}

struct Table {
    rows: Vec<(bool, Vec<Vec<Span>>)>,
    head: bool,
}

struct Layout {
    cols: usize,
    out: Vec<Line>,
    inline: Vec<Span>,
    flags: Vec<u8>,
    link: Option<String>,
    quote: u8,
    lists: Vec<List>,
    /// Indent of the current block's text.
    indent: usize,
    /// The current item's mark, for its first row.
    mark: Option<String>,
    heading: Option<u8>,
    code: Option<String>,
    /// The open code block's highlighter (a known fence language).
    hl: Option<Highlighter>,
    table: Option<Table>,
}

impl Layout {
    fn flags(&self) -> u8 {
        self.flags.last().copied().unwrap_or(0)
    }

    fn push_flag(&mut self, f: u8) {
        let n = self.flags() | f;
        self.flags.push(n);
    }

    fn text(&mut self, t: &str) {
        let t = clean(t);
        if t.is_empty() {
            return;
        }
        let flags = self.flags();
        let link = if flags & LINK != 0 { self.link.clone() } else { None };
        if let Some(Span { text, .. }) = self.inline.last_mut().filter(|s| s.flags == flags && s.link == link) {
            text.push_str(&t);
        } else {
            self.inline.push(Span { text: t, flags, link, hl: 0 });
        }
    }

    /// A blank row between blocks (not inside tight lists, not at the top).
    fn gap(&mut self) {
        if self.out.last().is_some_and(|l| !(l.spans.is_empty() && l.kind == Kind::Text)) && self.mark.is_none() {
            self.out.push(Line::new(Kind::Text, self.quote, 0));
        }
    }

    fn avail(&self) -> usize {
        self.cols.saturating_sub(self.quote as usize * 2 + self.indent).max(4)
    }

    /// Wrap the collected inline spans into rows.
    fn flush(&mut self, kind: Kind) {
        let spans = std::mem::take(&mut self.inline);
        let mark = self.mark.take();
        if spans.iter().all(|s| s.text.trim().is_empty()) && mark.is_none() {
            return;
        }
        let first_indent = self.indent - mark.as_ref().map_or(0, |m| width(m));
        let avail = self.avail();
        let mut rows: Vec<Vec<Span>> = vec![Vec::new()];
        let mut used = 0usize;
        for s in spans {
            for (i, para) in s.text.split('\n').enumerate() {
                if i > 0 {
                    rows.push(Vec::new());
                    used = 0;
                }
                for word in split_words(para) {
                    let is_space = word.chars().all(|c| c == ' ');
                    let ww = width(word);
                    if is_space {
                        if used > 0 && used + ww <= avail {
                            add(rows.last_mut().unwrap(), word, &s);
                            used += ww;
                        } else if used > 0 {
                            rows.push(Vec::new());
                            used = 0;
                        }
                        continue;
                    }
                    if used + ww > avail && used > 0 {
                        trim_end(rows.last_mut().unwrap());
                        rows.push(Vec::new());
                        used = 0;
                    }
                    // Longer than a row: hard breaks.
                    let mut rest = word.to_string();
                    while width(&rest) > avail - used {
                        let (head, tail) = split_at_width(&rest, avail - used);
                        if head.is_empty() {
                            rows.push(Vec::new());
                            used = 0;
                            continue;
                        }
                        add(rows.last_mut().unwrap(), &head, &s);
                        rows.push(Vec::new());
                        used = 0;
                        rest = tail;
                    }
                    if !rest.is_empty() {
                        used += width(&rest);
                        add(rows.last_mut().unwrap(), &rest, &s);
                    }
                }
            }
        }
        for (i, r) in rows.into_iter().enumerate() {
            let mut r = r;
            trim_end(&mut r);
            let mut l = Line::new(kind, self.quote, if i == 0 { first_indent } else { self.indent });
            if i == 0
                && let Some(m) = &mark
            {
                l.spans.push(Span::new(m.clone(), MUTED));
            }
            l.spans.extend(r);
            self.out.push(l);
        }
    }

    fn end_table(&mut self) {
        let Some(t) = self.table.take() else { return };
        let n = t.rows.iter().map(|r| r.1.len()).max().unwrap_or(0);
        if n == 0 {
            return;
        }
        let mut w = vec![1usize; n];
        for (_, r) in &t.rows {
            for (i, c) in r.iter().enumerate() {
                w[i] = w[i].max(c.iter().map(Span::width).sum());
            }
        }
        let avail = self.avail();
        let seps = 3 * (n - 1);
        // Too wide: shrink the widest columns first.
        while w.iter().sum::<usize>() + seps > avail {
            let (i, &m) = w.iter().enumerate().max_by_key(|(_, v)| **v).unwrap();
            if m <= 3 {
                break;
            }
            w[i] = m - 1;
        }
        for (ri, (head, r)) in t.rows.iter().enumerate() {
            let mut l = Line::new(Kind::Table, self.quote, self.indent);
            for (i, cw) in w.iter().enumerate() {
                if i > 0 {
                    l.spans.push(Span::new(" │ ", MUTED));
                }
                let cell = r.get(i).cloned().unwrap_or_default();
                let mut left = *cw;
                for mut s in cell {
                    if left == 0 {
                        break;
                    }
                    if s.width() > left {
                        let (h, _) = split_at_width(&s.text, left.saturating_sub(1));
                        s.text = format!("{h}…");
                    }
                    left -= s.width().min(left);
                    if *head {
                        s.flags |= BOLD;
                    }
                    l.spans.push(s);
                }
                if left > 0 && i + 1 < n {
                    l.spans.push(Span::new(" ".repeat(left), 0));
                }
            }
            self.out.push(l);
            if *head && (ri + 1 < t.rows.len()) {
                let rule = w.iter().map(|cw| "─".repeat(*cw)).collect::<Vec<_>>().join("─┼─");
                let mut l = Line::new(Kind::Table, self.quote, self.indent);
                l.spans.push(Span::new(rule, MUTED));
                self.out.push(l);
            }
        }
    }
}

fn add(row: &mut Vec<Span>, t: &str, like: &Span) {
    if let Some(last) = row.last_mut().filter(|l| l.flags == like.flags && l.link == like.link && l.hl == like.hl) {
        last.text.push_str(t);
    } else {
        row.push(Span { text: t.to_string(), flags: like.flags, link: like.link.clone(), hl: like.hl });
    }
}

fn trim_end(row: &mut Vec<Span>) {
    while let Some(l) = row.last_mut() {
        let t = l.text.trim_end_matches(' ').len();
        if l.flags & CODE != 0 && t > 0 {
            break;
        }
        l.text.truncate(t);
        if l.text.is_empty() {
            row.pop();
        } else {
            break;
        }
    }
}

/// Words and the runs of spaces between them.
fn split_words(s: &str) -> Vec<&str> {
    let mut v = Vec::new();
    let mut start = 0;
    let mut space = None;
    for (i, c) in s.char_indices() {
        let sp = c == ' ';
        if space.is_some_and(|p| p != sp) {
            v.push(&s[start..i]);
            start = i;
        }
        space = Some(sp);
    }
    if start < s.len() {
        v.push(&s[start..]);
    }
    v
}

/// The longest head of `s` at most `w` cells wide, and the rest.
fn split_at_width(s: &str, w: usize) -> (String, String) {
    let mut used = 0;
    for (i, c) in s.char_indices() {
        let cw = char_width(c);
        if used + cw > w {
            return (s[..i].to_string(), s[i..].to_string());
        }
        used += cw;
    }
    (s.to_string(), String::new())
}

/// Lay `src` out in rows at most `cols` cells wide. Unfinished markdown
/// (a reply still streaming in) lays out as far as it goes: an open code
/// fence runs to the end, an unclosed `**` shows as typed.
pub fn layout(src: &str, cols: usize) -> Vec<Line> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS);
    let mut st = Layout {
        cols: cols.max(8),
        out: Vec::new(),
        inline: Vec::new(),
        flags: Vec::new(),
        link: None,
        quote: 0,
        lists: Vec::new(),
        indent: 0,
        mark: None,
        heading: None,
        code: None,
        hl: None,
        table: None,
    };
    for ev in Parser::new_ext(src, opts) {
        match ev {
            Event::Start(t) => match t {
                Tag::Paragraph => {
                    if st.table.is_none() {
                        st.gap();
                    }
                }
                Tag::Heading { level, .. } => {
                    st.gap();
                    let n = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    };
                    st.heading = Some(n);
                    st.push_flag(BOLD);
                }
                Tag::BlockQuote(_) => {
                    st.flush(Kind::Text);
                    st.gap();
                    st.quote = st.quote.saturating_add(1);
                }
                Tag::CodeBlock(k) => {
                    st.flush(Kind::Text);
                    st.gap();
                    let info = match k {
                        CodeBlockKind::Fenced(l) => l.to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    st.hl = highlight::lang(&info).map(Highlighter::new);
                    st.code = Some(info);
                }
                Tag::List(start) => {
                    st.flush(Kind::Text);
                    if st.lists.is_empty() {
                        st.gap();
                    }
                    st.lists.push(List { next: start, base: st.indent });
                }
                Tag::Item => {
                    st.flush(Kind::Text);
                    let l = st.lists.last_mut();
                    let (m, base) = match l {
                        Some(List { next: Some(n), base }) => {
                            let m = format!("{n}. ");
                            *n += 1;
                            (m, *base)
                        }
                        Some(List { next: None, base }) => ("• ".to_string(), *base),
                        None => ("• ".to_string(), 0),
                    };
                    st.indent = base + width(&m);
                    st.mark = Some(m);
                }
                Tag::Emphasis => st.push_flag(ITALIC),
                Tag::Strong => st.push_flag(BOLD),
                Tag::Strikethrough => st.push_flag(STRIKE),
                Tag::Link { dest_url, .. } => {
                    st.link = Some(clean(&dest_url));
                    st.push_flag(LINK);
                }
                Tag::Image { .. } => {
                    st.push_flag(MUTED | ITALIC);
                    st.text("[image: ");
                }
                Tag::Table(_) => {
                    st.flush(Kind::Text);
                    st.gap();
                    st.table = Some(Table { rows: Vec::new(), head: false });
                }
                Tag::TableHead => {
                    if let Some(t) = st.table.as_mut() {
                        t.head = true;
                        t.rows.push((true, Vec::new()));
                    }
                }
                Tag::TableRow => {
                    if let Some(t) = st.table.as_mut() {
                        t.rows.push((false, Vec::new()));
                    }
                }
                Tag::TableCell => st.inline.clear(),
                _ => {}
            },
            Event::End(t) => match t {
                TagEnd::Paragraph => st.flush(Kind::Text),
                TagEnd::Heading(_) => {
                    let lvl = st.heading.take().unwrap_or(1);
                    st.flush(Kind::Heading(lvl));
                    st.flags.pop();
                }
                TagEnd::BlockQuote(_) => {
                    st.flush(Kind::Text);
                    st.quote = st.quote.saturating_sub(1);
                }
                TagEnd::CodeBlock => {
                    st.code = None;
                    st.hl = None;
                }
                TagEnd::List(_) => {
                    st.flush(Kind::Text);
                    if let Some(l) = st.lists.pop() {
                        st.indent = l.base;
                    }
                }
                TagEnd::Item => {
                    st.flush(Kind::Text);
                    st.mark = None;
                    st.indent = st.lists.last().map_or(0, |l| l.base);
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                    st.flags.pop();
                }
                TagEnd::Link => {
                    st.flags.pop();
                    st.link = None;
                }
                TagEnd::Image => {
                    st.text("]");
                    st.flags.pop();
                }
                TagEnd::TableCell => {
                    let cell = std::mem::take(&mut st.inline);
                    if let Some(r) = st.table.as_mut().and_then(|t| t.rows.last_mut()) {
                        r.1.push(cell);
                    }
                }
                TagEnd::TableHead => {
                    if let Some(t) = st.table.as_mut() {
                        t.head = false;
                    }
                }
                TagEnd::Table => st.end_table(),
                _ => {}
            },
            Event::Text(t) => {
                if st.code.is_some() {
                    let avail = st.avail().max(1);
                    for raw in t.trim_end_matches('\n').split('\n') {
                        let runs = match st.hl.as_mut() {
                            Some(h) => h.line(raw),
                            None => vec![(raw.to_string(), highlight::PLAIN)],
                        };
                        // Wrap the runs at `avail` cells, a row at a time.
                        let mut l = Line::new(Kind::Code, st.quote, st.indent);
                        let mut used = 0;
                        for (text, hl) in runs {
                            let mut rest = clean(&text);
                            while !rest.is_empty() {
                                if used >= avail {
                                    st.out.push(std::mem::replace(&mut l, Line::new(Kind::Code, st.quote, st.indent)));
                                    used = 0;
                                }
                                let (h, tail) = split_at_width(&rest, avail - used);
                                let (h, tail) = if h.is_empty() && used == 0 {
                                    // A character wider than the row: it goes on a row alone.
                                    let mut cs = rest.chars();
                                    let c = cs.next().map(String::from).unwrap_or_default();
                                    (c, cs.collect())
                                } else {
                                    (h, tail)
                                };
                                if h.is_empty() {
                                    used = avail;
                                    continue;
                                }
                                used += width(&h);
                                add(&mut l.spans, &h, &Span { text: String::new(), flags: CODE, link: None, hl });
                                rest = tail;
                            }
                        }
                        st.out.push(l);
                    }
                } else {
                    st.text(&t);
                }
            }
            Event::Code(t) => {
                st.push_flag(CODE);
                st.text(&t);
                st.flags.pop();
            }
            Event::Html(t) | Event::InlineHtml(t) => {
                st.push_flag(MUTED);
                st.text(t.trim_end_matches('\n'));
                st.flags.pop();
            }
            Event::InlineMath(t) | Event::DisplayMath(t) => {
                st.push_flag(CODE);
                st.text(&t);
                st.flags.pop();
            }
            Event::FootnoteReference(t) => st.text(&format!("[{t}]")),
            Event::SoftBreak => st.text(" "),
            Event::HardBreak => st.text("\n"),
            Event::Rule => {
                st.flush(Kind::Text);
                st.gap();
                st.out.push(Line::new(Kind::Rule, st.quote, st.indent));
            }
            Event::TaskListMarker(done) => {
                if let Some(m) = st.mark.as_mut() {
                    m.push_str(if done { "[x] " } else { "[ ] " });
                    st.indent += 4;
                }
            }
        }
    }
    st.flush(Kind::Text);
    while st.out.last().is_some_and(|l| l.kind == Kind::Text && l.spans.is_empty()) {
        st.out.pop();
    }
    st.out
}

/// Rows as terminal text: SGR styles, OSC 8 links (`links`), code rows
/// on a shaded background padded to `cols`. Ends every row with a reset.
pub fn to_ansi(lines: &[Line], cols: usize, links: bool) -> String {
    let mut s = String::new();
    for l in lines {
        for _ in 0..l.quote {
            s.push_str("\x1b[2m│\x1b[0m ");
        }
        s.push_str(&" ".repeat(l.indent as usize));
        match l.kind {
            Kind::Rule => s.push_str(&format!("\x1b[2m{}\x1b[0m", "─".repeat(cols.saturating_sub(l.start()).max(3)))),
            Kind::Code => {
                // Token colours are the terminal's own ANSI palette (its theme).
                let pad = cols.saturating_sub(l.width());
                for sp in &l.spans {
                    s.push_str(&format!("\x1b[0;48;5;236;{}m{}", code_sgr(sp.hl), sp.text));
                }
                s.push_str(&format!("\x1b[0;48;5;236m{}\x1b[0m", " ".repeat(pad)));
            }
            Kind::Heading(n) => {
                let sgr = match n {
                    1 => "1;4;35",
                    2 => "1;35",
                    _ => "1;36",
                };
                for sp in &l.spans {
                    s.push_str(&format!("\x1b[{sgr}{}m{}\x1b[0m", span_sgr(sp.flags & !BOLD), sp.text));
                }
            }
            Kind::Text | Kind::Table => {
                for sp in &l.spans {
                    let sgr = span_sgr(sp.flags);
                    let body = format!("\x1b[0{sgr}m{}\x1b[0m", sp.text);
                    match sp.link.as_ref().filter(|_| links) {
                        Some(u) => s.push_str(&format!("\x1b]8;;{u}\x1b\\{body}\x1b]8;;\x1b\\")),
                        None => s.push_str(&body),
                    }
                }
            }
        }
        s.push('\n');
    }
    s
}

/// Rows as plain text (output that is not a terminal).
pub fn to_text(lines: &[Line], cols: usize) -> String {
    let mut s = String::new();
    for l in lines {
        s.push_str(&"│ ".repeat(l.quote as usize));
        s.push_str(&" ".repeat(l.indent as usize));
        if l.kind == Kind::Rule {
            s.push_str(&"─".repeat(cols.saturating_sub(l.start()).max(3)));
        } else {
            s.push_str(l.text().trim_end());
        }
        s.push('\n');
    }
    s
}

/// SGR foreground for a code token class: ANSI palette slots, so the
/// terminal's theme picks the actual colours.
fn code_sgr(hl: u8) -> &'static str {
    match hl {
        highlight::KEYWORD => "35",
        highlight::TYPE => "33",
        highlight::STRING => "32",
        highlight::NUMBER => "36",
        highlight::COMMENT => "3;90",
        highlight::FUNC => "34",
        highlight::ADDED => "32",
        highlight::REMOVED => "31",
        highlight::META => "1;36",
        _ => "38;5;223",
    }
}

fn span_sgr(f: u8) -> String {
    let mut s = String::new();
    if f & BOLD != 0 {
        s.push_str(";1");
    }
    if f & ITALIC != 0 {
        s.push_str(";3");
    }
    if f & STRIKE != 0 {
        s.push_str(";9");
    }
    if f & MUTED != 0 {
        s.push_str(";2");
    }
    if f & LINK != 0 {
        s.push_str(";4;34");
    }
    if f & CODE != 0 {
        s.push_str(";48;5;236;38;5;223");
    }
    s
}

// ---- Panel items (chat transcripts, previews): the host draws the styles
// (`PanelStyle::Md` and friends) in the theme's colours.

use crate::ui::{PanelItem, PanelStyle, Rect};

/// Items for one row at (`x`, `y`), `w` px wide. `prev_code` / `next_code`:
/// whether the rows around it are code too (one rounded box per block).
/// `link` makes the click action of a link span.
#[allow(clippy::too_many_arguments)]
pub fn line_items<A: Default>(
    l: &Line,
    (prev_code, next_code): (bool, bool),
    x: i32,
    y: i32,
    (cw, row_h): (i32, i32),
    w: i32,
    link: &mut dyn FnMut(&str) -> A,
    out: &mut Vec<PanelItem<A>>,
) {
    let it = |rect, text: String, style| PanelItem::<A> { rect, text, style, action: A::default() };
    for q in 0..l.quote as i32 {
        out.push(it(Rect { x: x + q * 2 * cw + cw / 3, y, w: 3, h: row_h }, String::new(), PanelStyle::MdQuote));
    }
    let sx = x + l.start() as i32 * cw;
    let right = x + w;
    match l.kind {
        Kind::Rule => {
            out.push(it(Rect { x: sx, y, w: right - sx, h: row_h }, String::new(), PanelStyle::MdRule));
            return;
        }
        Kind::Code => {
            out.push(it(
                Rect { x: sx - cw / 2, y, w: right - sx + cw / 2, h: row_h },
                String::new(),
                PanelStyle::MdCode { top: !prev_code, bottom: !next_code },
            ));
        }
        _ => {}
    }
    let heading = if let Kind::Heading(n) = l.kind { n } else { 0 };
    let mut cx = sx;
    for s in &l.spans {
        if cx >= right {
            break;
        }
        let sw = width(&s.text) as i32 * cw;
        // Code block text is plain on the block's background.
        let flags = if l.kind == Kind::Code { 0 } else { s.flags };
        let action = match (&s.link, s.flags & LINK != 0) {
            (Some(u), true) => link(u),
            _ => A::default(),
        };
        let style = if l.kind == Kind::Code && s.hl != 0 { PanelStyle::MdToken(s.hl) } else { PanelStyle::Md { flags, heading } };
        out.push(PanelItem { rect: Rect { x: cx, y, w: sw.min(right - cx), h: row_h }, text: s.text.clone(), style, action });
        cx += sw;
    }
}

/// Items for `lines` from (`x`, `y`), one row each `row_h` px, at most
/// `rows` rows from `skip`. Returns the rows used.
#[allow(clippy::too_many_arguments)]
pub fn items<A: Default>(
    lines: &[Line],
    skip: usize,
    rows: usize,
    x: i32,
    y: i32,
    cell: (i32, i32),
    w: i32,
    link: &mut dyn FnMut(&str) -> A,
    out: &mut Vec<PanelItem<A>>,
) -> usize {
    let code = |i: usize| lines.get(i).is_some_and(|l| l.kind == Kind::Code);
    let mut n = 0;
    for (i, l) in lines.iter().enumerate().skip(skip).take(rows) {
        let around = (i > skip && code(i - 1), i + 1 < skip + rows && code(i + 1));
        line_items(l, around, x, y + n as i32 * cell.1, cell, w, link, out);
        n += 1;
    }
    n
}
