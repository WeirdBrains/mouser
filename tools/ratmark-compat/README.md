# ratmark compatibility harness

Lays the same documents out with Mouser's Markdown renderer
(`mouser-mode-api`, feature `markdown`) and with
[ratmark](https://github.com/nixpulvis/ratmark), at the same widths, and
measures where they differ. It is its own Cargo workspace: Mouser's build and
dependencies do not change.

Two ratmark revisions are compared, both pinned to exact commits in
`Cargo.toml`:

- `ratmark_base`: upstream `nixpulvis/ratmark` at `6483e72`.
- `ratmark`: the WeirdBrains contribution branch under test
  ([WeirdBrains/ratmark](https://github.com/WeirdBrains/ratmark)).

```sh
cd tools/ratmark-compat
cargo test                    # the checks in tests/compat.rs
cargo run --release > report.md
```

Use a temporary `HOME`/`MOUSER_HOME` and `CARGO_TARGET_DIR` as for any Mouser
build; nothing here reads Mouser's settings.

## What is measured

- **Rows**: each renderer's rows as plain text (`markdown::to_text` for Mouser,
  `Rendered::render(…, Theme::plain())` for ratmark), trailing spaces trimmed.
- **Rows equal to Mouser's** and **docs with Mouser's words**: how close the look
  and the content are. Marks (`│ ─ • ‹ › …`) are dropped before comparing words.
- **Over width**: rows wider than the layout width, by Mouser's grid rule
  (`markdown::width`) and by unicode-width's string rule (what Ratatui uses).
- **Grapheme splits**: row breaks inside a grapheme cluster (a ZWJ emoji, a
  flag, a letter with combining marks).
- **Control characters**: rows that still hold one (an escape sequence could
  reach a terminal).
- **Streaming**: the document laid out at growing prefixes. `back` is how many
  rows of one prefix's layout changed or disappeared in the next, counted from
  the end; 1 is the row being written. This measures what each renderer does,
  it is not a promise that earlier rows never move: a fence or a table header
  arriving legitimately re-lays the block it starts.

`tests/compat.rs`: `fix_*` checks hold for the contribution branch; `gap_*`
checks record a difference from Mouser that is still open, so a change on
either side fails them and gets noticed.
