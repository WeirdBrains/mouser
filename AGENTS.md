# AGENTS.md — rules for agents working on Mouser

These rules are adapted from the owner's Muninn project for this repository.

## Where to work
- Always work directly on the owner's machine (alien-blue), in this repository folder:
  `~/Projects/WeirdBrains/mouser`. No cloud agents, no clones elsewhere. Work on `main` directly; no PRs or branches.
- Toolchain: Rust >= 1.99 (edition 2024), cargo on this machine. Prefer bundled/vendored
  crate features over installing system packages; ask before installing anything.

## Source of truth
- Front ends are thin layers over the library; no business logic in them. Menus,
  keybindings and help dispatch the same named actions. If Mouser grows a CLI or help
  view, they remain thin adapters over the core library.

## Safety
- Never touch the user's real Mouser home for tests or manual runs. Use a temp
  directory (set `MOUSER_HOME` or pass `--config-dir`) for all non-release runs.
- Never edit files outside this project folder (cargo caches excepted).
- Git: commit locally in logical steps. Work on `main` directly. Never force-push, rewrite
  published history, or skip hooks.
- Secrets (keys, tokens, passwords) are never logged, printed in `-v` output, or committed.

## Versioning and releases
- Semantic versioning. Pre-1.0: MINOR bump per new feature, PATCH for fixes/docs-only.
- Each release: bump `Cargo.toml` (+ `Cargo.lock`), update the version shown in the app
  (if displayed), add a summary-level `CHANGELOG.md` entry, commit as `release: vX.Y.Z`,
  create an annotated tag `vX.Y.Z`, then push `main` with the tag:  
  `git push origin main --follow-tags`.
- Releases use the shared WeirdBrains release script (weirdnet-project-helpers), vendored as
  `scripts/weirdnet_release.py` and configured by `release.toml`:
  `python3 scripts/weirdnet_release.py [--major|--patch] "message"` does all of this (MINOR by
  default); the `v*` tag push is what starts the Release workflow. See `docs/release.md`. Don't
  edit the vendored copy; change weirdnet-project-helpers and re-vendor it.
- Every release ends with the TAUR upload: after the Release workflow's `packaging:`
  commit, `git pull` and `python3 scripts/weirdnet_release.py taur X.Y.Z` (taurctl signed in).
- `CHANGELOG.md` (Keep a Changelog format) stays summary level: a few short one-line
  bullets per release naming the feature. No implementation details or file lists.

## Documentation
- Every feature is documented in `docs/` (one Markdown file per feature, indexed in
  `docs/README.md`). Update docs in the same change as the code.
- In-app help (if present) is built from `docs/` (single source). Topics should be
  searchable and cross-linked.
- Every **public** Markdown doc in `docs/` must have a matching page on the website, in
  the right section (User Guides, Developer Guides, MCP Guides or API Guides), updated in
  the same change. Map each new doc in `docs/sections.toml` to `user`, `developer`, `mcp`,
  `api`, or `internal`. Internal docs (maintainer/agent notes such as deploy-site, the
  release process and HANDOVER) stay in the repo and are not published to the site. CI
  (`scripts/check-doc-sections.py`) fails on an unmapped doc, and the Docs site workflow
  rebuilds the site's pages from `docs/` on a `v*` tag or a `main` push that touches the
  site or docs (skipping `internal`; also runnable by hand).
- If Mouser exposes a CLI, generate man pages from clap definitions (clap_mangen) into
  `man/`, with additional FILES / ENVIRONMENT / EXIT STATUS / SECURITY / EXAMPLES sections.

## Quality bar (before every commit)
- `cargo fmt --check`, `cargo build`, `cargo clippy --all-targets` (no warnings),
  and `cargo test` all pass.
- New behaviour ships with tests.

## Fixed design decisions for Mouser
- Scrollback compression: idle lines are compressed into chunked blocks (zstd or lz4).
- Decompression on demand: scrolling or search transparently decompresses needed chunks,
  with prefetch around the viewport; chunks re-compress after going idle again.
- Full Kitty protocol support is a long-term goal (keyboard, graphics, and common
  extensions). Track status in `docs/kitty-compliance.md`.
- Configuration is TOML-based (font, colors, scrollback, compression settings).
- Storage lives in `~/.mouser` by default with `MOUSER_HOME` override. Built-in themes are
  provided and written on first run or copied on edit.

## Releases
- For each tested change: bump versions (docs-only: PATCH; feature: MINOR), update `CHANGELOG.md`,
  commit as `release: vX.Y.Z`, create an annotated tag `vX.Y.Z`, then push:
  `git push origin main --follow-tags`. Packaging ships prebuilt binaries; installing must not require Rust.

## Communication style
- Keep summaries, changelog entries and commit messages concise.

