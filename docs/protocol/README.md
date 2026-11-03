# Fuzzball protocol

The Fuzzball spec now lives with the [`fuzzball`](https://crates.io/crates/fuzzball) crate,
which Mouser uses for every Fuzzball encode and parse:

- Site: <https://weirdbrains.github.io/fuzzball/>
- Repo: <https://github.com/WeirdBrains/fuzzball>, spec in [`docs/`](https://github.com/WeirdBrains/fuzzball/tree/main/docs)
  - [`README.md`](https://github.com/WeirdBrains/fuzzball/blob/main/docs/README.md): framing, capabilities, versioning, errors
  - [`graphics.md`](https://github.com/WeirdBrains/fuzzball/blob/main/docs/graphics.md): images (upload, place, delete, query)
  - [`keyboard.md`](https://github.com/WeirdBrains/fuzzball/blob/main/docs/keyboard.md): structured key events
  - [`plugins.md`](https://github.com/WeirdBrains/fuzzball/blob/main/docs/plugins.md): plugin handshake
  - [`shell.md`](https://github.com/WeirdBrains/fuzzball/blob/main/docs/shell.md): silent shell actions
  - [`wire-format.md`](https://github.com/WeirdBrains/fuzzball/blob/main/docs/wire-format.md): exact bytes Mouser sends

References in Mouser's code to `fuzzball docs/<file>.md §N` point at those files.
Mouser's terminal side (upload assembly, image store, placements, keyboard mode per
screen, plugin grants) is in `crates/mouser-core/src/fuzzball.rs`.
