# Mouser

This file adds Mouser-specific rules on top of the collective
`WeirdBrains/weirdnet-agents` `AGENTS.md`
(<https://github.com/WeirdBrains/weirdnet-agents>). That file is mandatory.
If the two conflict, the collective file wins unless this file explicitly
overrides a rule and gives a reason.

Shared rules live only there: straight to `main` with no pull requests, the
quality bar, secrets, temp homes (`MOUSER_HOME`), virtual-display GUI tests on
alien-blue, the release script, and changelog style. Do not copy them here.

## Where to work

- This checkout is `~/Projects/WeirdBrains/mouser` on alien-blue.

## Documentation

- One Markdown file per feature in `docs/`, indexed in `docs/README.md`. In-app
  help is built from those files.
- Map each public doc in `docs/sections.toml` to `user`, `developer`, `mcp`,
  `api`, or `internal`. Internal docs (deploy-site, the release process,
  HANDOVER) stay in the repo. `scripts/check-doc-sections.py` fails on an
  unmapped doc. The Docs site workflow rebuilds public pages on a `v*` tag or
  a `main` push that touches the site or `docs/`.
- Man pages are generated from clap (`mouser man --out man`), with FILES,
  ENVIRONMENT, EXIT STATUS, SECURITY and EXAMPLES on `mouser(1)`.

## Releases

Do not cut a release or create a tag unless the owner asks. Record the change
under `## [Unreleased]` and leave the version alone.

What this repo does (`docs/release.md`, `release.toml`, helpers 0.1.1), beyond
the collective release rules:

- The script bumps `[workspace.package] version` and the members `release.toml`
  follows, writes `Cargo.lock` when present (gitignored, so not committed), the
  version lines in `man/*.1`, `Current release` in `docs/HANDOVER.md`, and the
  homepage version in `site_jaspr/lib/pages/home.dart`. It commits
  `release: vX.Y.Z`, tags `vX.Y.Z`, and pushes `main` and that tag atomically.
  It then publishes `mouser-engine-abi`, `mouser-views` and `mouser-media` when
  that version is not on crates.io yet, asking first.
- A push to `main` starts neither `.github/workflows/release.yml` nor `ci.yml`.
  Only the `v*` tag does. `workflow_dispatch` on the Release workflow builds
  and publishes nothing. The publish job runs only on a `v*` tag.
- The workflow builds Linux x86_64, macOS x86_64 and arm64, and Windows
  x86_64. Packages: `.deb` and `.rpm` (nfpm), Arch `mouser-bin`
  `.pkg.tar.zst`, macOS `Mouser.app` and `.dmg`, Windows `.zip` and
  `MouserSetup-<ver>.exe` (Inno Setup; not WiX). Engines ship beside the
  binary (`mouser engine bundle`). Linux desktop files are validated.
- After the release is published the workflow pushes a `packaging:` commit:
  `Formula/mouser.rb`, `Casks/mouser.rb`, and `packaging/arch/mouser-bin/`.
- TAUR is required and is not in the workflow. After that commit is on `main`,
  on a machine with `taurctl` signed in: `git pull`, then
  `python3 scripts/weirdnet_release.py taur X.Y.Z` (org WeirdBrains, package
  `mouser-bin`). A release is not finished until that has run.
- Gaps, as the repo stands: minisign is set (`OFFICIAL_KEY`); macOS and Windows
  certificate secrets are not, so those builds stay unsigned. `-full` offline
  bundles are not built (`plugins.lock` is empty). aarch64 Linux is not set
  up. There is no ARM64 Windows build. Android and iOS are opted out. AUR
  `yay` install is still planned.

## Plugins, PTYs and the tabs decoration

- Plugin kinds: mode, engine, model-provider, tool-provider, command,
  decoration. Manifests are `plugin.toml`. State is
  `$MOUSER_HOME/plugins/state.toml`. API version is 1. Official modes are out
  of process. Engines are native libraries. The contract is `docs/plugins.md`.
- A decoration draws the window chrome (draw ops and hit targets) and asks the
  host to spawn, close or focus PTYs. The host owns the OS PTY. Every enabled
  decoration runs. Registry order is priority (first is highest). The
  scheduler stacks chrome into disjoint bands and merges PTY ops: spawns all
  apply, and the highest priority wins a focus/close conflict. `tabs` is
  built in and off until `mouser plugin enable tabs`. With it off, and with
  no other decoration, the built-in tab bar (shown from the second tab) and
  one login shell per tab stay as they are. With only `tabs` on, behavior
  matches that one decoration.
- External decorations use `transport = "decoration"` and newline JSON
  (`hello` / `chrome` / `event` / `note`, reply `{chrome, ops}`). They need
  `run` allowed. A reply over 500 ms, a bad reply, or a panic drops that
  decoration only. If none remain, the window uses the built-in sessions.
  The Rust trait is `mouser_core::decoration::Decoration`. `compose`,
  `arbitrate` and `fanout_hit` are the scheduler.
- Host glue is `mouser-app` (`decor.rs`, `tabs.rs`). Chrome insets shrink the
  grid. Only the active PTY is resized, and a newly focused PTY is resized to
  the stored size. On Unix the session server forwards chrome over the split
  protocol (`mouser-render` `split.rs`, protocol 2). Lock tabs, then the
  decoration.
- `[tabs] shell` is the default for a new tab. `[[tabs.tab]]` is file-only
  (shell or command per tab). See `docs/tabs.md`.
- Tests that do not need a window: `cargo test -p mouser-core decoration`,
  `cargo test -p mouser-app decor`, `cargo test -p mouser-pty`. Kill any shell
  a PTY test spawned. `MOUSER_UI=headless` runs the session server with no
  window. A winit window follows the collective virtual-display rule.

## Fixed design

- Idle scrollback lines are compressed into chunked blocks (zstd or lz4) and
  decompressed on demand, with prefetch around the viewport.
- Full Kitty protocol support is a long-term goal (keyboard, graphics, and
  common extensions). Status is `docs/kitty-compliance.md`.
- Configuration is TOML. Storage defaults to `~/.mouser`, overridable with
  `MOUSER_HOME`. Built-in themes are written on first run or copied on edit.
