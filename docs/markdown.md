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

## What is drawn

| Markdown | Drawn as |
|----------|----------|
| `# Heading` | bold; levels 1–2 in the theme accent, level 1 underlined |
| `**bold**`, `*italic*`, `~~strike~~` | bold, slanted, struck through |
| `` `code` `` | on a small rounded shade |
| fenced / indented code | monospaced on a rounded background box |
| `> quote` | an accent bar per level |
| lists, `- [x]` tasks | `•` or `1.` marks, nested lists indented |
| `[text](url)` | underlined in the accent |
| tables | aligned columns with `│` and a rule under the header; wide tables shrink, cells end in `…` |
| `---` | a rule across the width |
| `![alt](img)` | an `[image: alt]` placeholder |

Text wraps at word boundaries to the width it is shown at (long words break).
Everything is one cell grid with the terminal font, so previews use the same face
and theme colours as the rest of Mouser.

## How it works

`mouser_core::markdown` parses with [pulldown-cmark](https://crates.io/crates/pulldown-cmark)
and lays the document out as rows of styled spans (bold, italic, code, link, strike,
muted; heading level, quote depth, indent). Each surface draws those rows its own way:
the frame draws them as panel items in the theme's colours (commander, chat).

Control characters in the text are shown as `�`, never passed through.
