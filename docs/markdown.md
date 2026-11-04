# Markdown rendering

Mouser renders Markdown (CommonMark with tables, ~~strikethrough~~ and task lists)
instead of showing the raw markup:

- **Commander previews**: `.md`, `.markdown`, `.mdown` and `.mkd` files in the grid,
  the carousel and the file view ([Modes](./modes.md)).
- **Chat**: your messages and the model's replies ([Chat](./chat.md)). Links are
  clickable (web, mail and `file://` only, never through a shell). A reply still
  streaming in is laid out as far as it has come: an open code fence is code to the
  end, an unclosed `**` shows as typed until it closes, and the rows above the one
  being written do not move.
- **The terminal**: `mouser md FILE…` (or `mouser md -`, or piped stdin) prints a
  file formatted, see [below](#in-the-terminal-mouser-md).

## What is drawn

| Markdown | Drawn as |
|----------|----------|
| `# Heading` | bold; levels 1–2 in the theme accent, level 1 underlined |
| `**bold**`, `*italic*`, `~~strike~~` | bold, slanted, struck through |
| `` `code` `` | on a small rounded shade |
| fenced / indented code | monospaced on a rounded background box; fenced blocks with a known language are syntax highlighted ([Code](#code-blocks)) |
| `> quote` | an accent bar per level |
| lists, `- [x]` tasks | `•` or `1.` marks, nested lists indented |
| `[text](url)` | underlined in the accent |
| tables | aligned columns with `│` and a rule under the header; wide tables shrink, cells end in `…` |
| `---` | a rule across the width |
| `![alt](img)` | an `[image: alt]` placeholder |

Text wraps at word boundaries to the width it is shown at (long words break).
Everything is one cell grid with the terminal font, so previews use the same face
and theme colours as the rest of Mouser.

## Code blocks

A fenced block's language tag picks the highlighter (only the first word counts,
so `rust,ignore` and `py title=x.py` work):

| Language | Tags |
|----------|------|
| Rust | `rust`, `rs` |
| Python | `python`, `py` |
| JavaScript / TypeScript | `js`, `jsx`, `ts`, `tsx`, `javascript`, `typescript` |
| Shell | `sh`, `bash`, `zsh`, `shell`, `console`, `fish` |
| JSON, TOML, YAML | `json`, `toml`, `yaml`, `yml` |
| Go | `go` |
| C / C++ | `c`, `h`, `cpp`, `c++`, `cc`, `hpp` |
| HTML / XML, CSS | `html`, `xml`, `svg`, `css`, `scss` |
| SQL | `sql` |
| Markdown, diffs | `md`, `markdown`, `diff`, `patch` |

Any other tag, or none, stays plain monospace. Keywords, types, strings, numbers,
function calls, comments and diff `+`/`-` lines take colours from the active
theme's ANSI palette (the bright slots on dark themes, the normal ones on light
themes; comments are muted text), so every theme brings its own highlighting.
`mouser md` writes the same classes as ANSI colours, which your terminal's theme
resolves.

The highlighter is a small built-in lexer (no extra dependencies). It works line
by line and only carries state forward (an open `/* comment */`), so a code block
that is still streaming in never recolours the rows above it.

## In the terminal: `mouser md`

```sh
mouser md README.md            # one file
mouser md a.md b.md            # one after another
curl -s …/notes.md | mouser md # stdin (or `mouser md -`)
mouser md --width 72 NOTES.md  # lay out at 72 columns
mouser md --color never x.md   # plain text, no escapes
```

The output is ordinary text with SGR styles (bold, italic, underline, colours, a
background behind code blocks) and OSC 8 hyperlinks, so it lands in the grid and the
scrollback like any other output: scroll back with the wheel or `shift+pageup`, select
and copy it, run it over SSH or inside tmux, in Mouser or any other terminal.

- `--width N`: columns to lay out at; the default is the terminal's width (then
  `COLUMNS`, then 80).
- `--color auto|always|never`: `auto` (default) styles the output only when stdout is
  a terminal, so `mouser md x.md > out.txt` writes plain wrapped text.

Links are OSC 8: Mouser makes them clickable (ctrl+click), other terminals that
support OSC 8 do the same, and the rest show the link text.

## How it works

`mouser_core::markdown` parses with [pulldown-cmark](https://crates.io/crates/pulldown-cmark)
and lays the document out as rows of styled spans (bold, italic, code, link, strike,
muted; heading level, quote depth, indent). Each surface draws those rows its own way:
the frame draws them as panel items in the theme's colours (commander, chat), and
`mouser md` writes them as SGR-styled text (`markdown::to_ansi`).

Control characters in the text and in link targets are shown as `�` (or dropped),
never passed through, so a file cannot send escape sequences to your terminal.
