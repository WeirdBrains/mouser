# Mouser handover notes

Repository: https://github.com/WeirdBrains/mouser (moved from m3talsmith/mouser on 2026-10-31). Current release: v0.7.2; main has unreleased work (see CHANGELOG Unreleased). Homebrew formula/cask are updated automatically by the Release workflow. CI covers Linux, macOS and Windows.

Public notes for whoever picks up Mouser next. Machine-specific paths, local
branch/worktree state and other private context live in `HANDOVER.local.md`
at the repo root (gitignored; ask the maintainer if you don't have it).

## Features and how to test

| Feature | What it does | How to test |
|---|---|---|
| Shell exit / Ctrl+D | Tab/window closes when the shell exits | Run the app, press Ctrl+D |
| Tabs | Each tab has its own PTY, screen, scrollback, title. Ctrl+Shift+T new, Ctrl+Shift+W close, Ctrl+Tab / Ctrl+Shift+Tab cycle, Alt+1..9 jump | Open several tabs |
| Connections sidebar | Hosts come from your ssh config; clicking one opens `ssh <alias>` in a new tab | Set `MOUSER_SSH_CONFIG` to a temp config |
| Sidebar state | Starts collapsed unless `MOUSER_OPEN_SIDEBAR` is set; remembers last open/closed state | Toggle, restart (use a temp `HOME`) |
| Event-driven redraws | `ControlFlow::Wait` + `EventLoopProxy`, glyph cache; ~0% idle CPU | Watch `top` while idle |
| GPU renderer | wgpu glyph atlas drawn as instanced quads; `MOUSER_RENDERER=cpu` forces the softbuffer fallback | Run both ways and compare |
| Font picker | Click the font field to list installed families alphabetically; typing fuzzy-filters | Appearance panel |
| Kitty keyboard | Only Kitty protocol; fallback behind Fuzzball keyboard. Matrix in `docs/kitty-compliance.md` | `kitten show-key -m kitty` |
| Fuzzball | v0.1: graphics (upload/place/delete/query) + keyboard mode, `crates/mouser-core/src/fuzzball.rs` | `tools/fuzzball-demo.py` inside Mouser |
| xterm / ECMA-48 | SGR (truecolour, curly/coloured underline), wide chars, tab stops, modes, DA/DSR, mouse, focus, bracketed paste, OSC titles, OSC 8 links (Ctrl+click), OSC 52, window reports 11/13/20/21t | `cargo test --workspace` + manual |
| vttest fixes | DEC line drawing, SI/SO, DECSCNM, LNM, DECREQTPARM, DA1 | Run `vttest` inside Mouser |
| Run a command | `mouser -e cmd args`, `mouser -- cmd args`, `--working-directory`/`--dir`, `--title`, `--class`, `--hold`, `--version`; desktop entry has xdg-terminal-exec keys so it can be a default terminal (e.g. on Omarchy) | `mouser -e btop`, `xdg-terminal-exec btop` |
| terminfo | `tic -x -o ~/.terminfo extra/mouser.terminfo`, then `TERM=mouser` | `infocmp mouser` |

Verified live (wtype, 2026-10-30): Fuzzball key events, Kitty keyboard flags 1/9/27/31 and `CSI ? u`, vttest menu 5 cursor keys (normal + application) and function keys F1–F12 (F10 is Mouser's command line, so it never reaches the app; F13+ send nothing in legacy mode). Clicking a sidebar connection opens it in a new tab (ydotool click; a failed ssh shows "[process exited 255] press any key to close" and a key closes the tab). wtype uses a virtual keymap, so `code=` reads `Escape` and Shift does not change the character; physical codes need a real keyboard.
Not yet verified live: vttest menu 5 subtests other than cursor and function keys, and the cause of vttest's LNM "Not expected".

## Known bugs
- (Fixed in 0.7.1: dashed box-drawing lines, now drawn procedurally with one shared integer geometry (light, heavy and double lines) in `crates/mouser-render/src/boxdraw.rs`, identical on CPU and GPU; the "81 wide" vttest report was the window really being 81 columns, covered by a regression test.)
- Website install code blocks clip long commands (scroll sideways).

## Known gaps
DECSLRM · colour emoji · real italic face (synthetic now) · visible blink ·
DECCOLM 132-column · G2/G3/national charsets · protected areas · ENQ answerback ·
OSC 133 UI.

## Fuzzball
Spec v0.1 with resolved decisions in `docs/protocol/README.md` §10. OSC 1337 and xterm-kitty
were removed. Kitty keyboard and Kitty graphics are fallbacks; Fuzzball takes precedence.
Placements follow their text (scroll regions, IL/DL, scrollback, resize) and are removed
when their anchor row leaves the scrollback; abandoned uploads expire on a timer
(`Session::watch_graphics` in `crates/mouser-app/src/tabs.rs`, which also plays Kitty
animations and is kicked through a condvar when new timer work arrives). Resize scales placements to the screen width, shrinking in place (`Placement::fit`, requested
col/cols/rows kept) and `ImageScaler` caches area-averaged downscales for both renderers.
Decisions: protocol README §10 #10–15.
`mouser show` (crates/mouser-app/src/show.rs, tested in crates/mouser-app/tests/show_tests.rs) displays images: Fuzzball first, Kitty fallback, Kitty Unicode placeholders inside tmux (README §10 #19-20).

## Kitty graphics
`crates/mouser-core/src/kitty_graphics.rs` (own `Graphics` store with `kitty: true`, shared
lifecycle methods in `graphics.rs`, APC interception in `TerminalParser::feed`), tests in
`crates/mouser-core/tests/kitty_graphics_tests.rs`. Renderer layers (`frame.rs`): z below
`KITTY_BELOW_BG` under cell backgrounds, other negative z between backgrounds and text.
Placeholders are drawn by `KittyGraphics::placeholder_draws` from the live screen cells
(three combining marks per cell). Supported set and deliberate limits: graphics.md §7.
Not yet done: IME events, lock-key state.

## Website
- Lives on the `site` branch in `site/` (plain HTML/CSS/JS).
- `.github/workflows/pages.yml` deploys GitHub Pages on pushes to `site`.
- Sections alternate light/dark with subtle gradients, starting light (hero).
- Rounded corners use `--radius: 20px` in `site/style.css` (cards, code, buttons, images, tables).
- To do: confirm feature claims (compressed scrollback, Windows builds, Homebrew
  macOS); retake `site/assets/mouser-app.png` with an opaque background.

## Release process (see AGENTS.md)
1. Bump version and update CHANGELOG.
2. Commit `release: vX.Y.Z`, create an annotated tag `vX.Y.Z`.
3. `git push origin main --follow-tags`.
4. Check CI with `gh run list` / `gh run watch`.
Releases happen only with the maintainer's explicit approval.

## Rules
- Work on `main`, no PRs/branches (exception: `site` for the website).
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`
  and `cargo test --workspace` must pass.
- Never touch the real `~/.mouser`; never force-push; ask before adding dependencies.
- Tests use `MOUSER_SSH_CONFIG` and a temp `HOME`, never the real `~/.ssh`.

## Ideas in progress
`MODES.md` (repo root) is the maintainer's early draft of interaction modes:
commander, dj, director, debugger, artist. Switch with `esc :` plus the mode's key
(e.g. `esc :c`), `ctrl-shift-k` shows mappings, `ctrl-shift-m` lists modes.
Commander shows a scrollable file list above the prompt after `cd`; `t` cycles views.
Only commander is described so far; wait for the maintainer before building it.

## Packaging
CI may only use GitHub-owned or verified actions (WeirdBrains org policy); any other
action fails the run at startup with no jobs. The workflows install Rust with `rustup`,
cache with `actions/cache` and publish with `gh release create` (2737875); the release
path is untested until the next tag.
Arch: `packaging/arch/mouser-git` (source) and `packaging/arch/mouser-bin` (release
binary) PKGBUILDs with `.SRCINFO`; details and AUR steps in `docs/release.md`. Both
conflict with each other and replace older `mouser-bin` installs. The release workflow
builds the `mouser-bin` `.pkg.tar.zst` and then commits real checksums via
`packaging/arch/update-bin.sh`. AUR publishing waits: registration is closed and it needs
the maintainer's account; the repo also needs a `LICENSE` file first (namcap error).
All URLs now point at WeirdBrains/mouser; the app ID `io.github.m3talsmith.mouser`
stays unless the maintainer decides otherwise.

## Recent work on main (since v0.7.2)
- Kitty graphics fallback: 8332289, 5628141, 529c968, 75e5bcf, docs f9212d7.
- Command-line launching and xdg-terminal-exec desktop entry: 88e50c1; CI fixes
  2737875 (actions policy), 5da1891 (non-Linux warning), 4b70ea7 (Windows test).
- Arch packaging (mouser-git, mouser-bin) and URL move to WeirdBrains: see Packaging.
- Selection, copy and paste: 3acc2a1 (model `mouser-core/src/selection.rs`, app glue
  `mouser-app/src/select.rs`, renderer hooks), c21b8c0 (paste sanitising, Shift+Insert),
  022c8a6 (smithay-clipboard: Hyprland ignores `wl-copy --primary` over data-control, so
  PRIMARY/clipboard are set through Mouser's own Wayland connection; auto-scroll keeps
  going when the compositor stops sending motion after the pointer leaves). Live-verified
  on Hyprland (word, line, drag+copy, block, auto-scroll into scrollback, Ctrl+Shift+V and
  middle-click paste into `cat`, clear on typing). Gaps: no Windows clipboard; X11 PRIMARY
  needs xclip/xsel.
- Images always fit the width (t132u): `fit=w` Fuzzball key, `mouser show` fills the
  width and follows resizes; overhanging placements move to column 0 (no more 1-column pin).
- Re-wrap on resize (t133u): `mouser-core/src/reflow.rs` (Layout: join soft-wrapped rows,
  track points, split), `TerminalParser::reflow` (scrollback + main screen, placements,
  empty-row adjustment for images whose height changes), `Screen::finish_reflow`,
  `Scrollback::replace_all`. `attr::WRAP_PAD` marks the blank left when a wide char wraps
  early. Tests: `mouser-core/tests/reflow_tests.rs` (perf: `--release -- --ignored`).
  Fuzzball spec: `g_fit=w`, `g_reflow=1` in the capability reply; graphics.md §2.2/§3/§4,
  README §10 #12/#13/#20/#21.
  Edge cases left: scrollback keeps plain text only (attributes/links lost there, as before);
  a line longer than the scrollback limit after narrowing drops oldest lines; an anchor
  in the blanks before the cursor moves with the cursor's padding; Kitty placeholder text
  is re-wrapped as text (an image wider than the new width breaks into rows); rows-only
  resizes do not re-wrap (nothing to do) or pull lines back from the scrollback.
- Multi-image invariant tests: `mouser-core/tests/multi_image_tests.rs` (t135u). Lazy
  scrollback re-wrap (t136u): `TerminalParser::reflow_step`, `reflow_sync_rows`
  (`REFLOW_SYNC_ROWS` 2000, `REFLOW_STEP_ROWS` 8192), worker in `Session::settle_reflow`,
  `Scrollback::splice`/`line_start`/`lines_wrapped`. Bench: `cargo test --release -p
  mouser-core --test reflow_tests perf -- --ignored --nocapture` (`SYNC=1000000` = old
  full re-wrap). Live sweep harness /tmp/mx/img/sweep.sh. Known: an occasional 0.3–1 s
  frame at the start/end of a Hyprland resize is spent blocked in present (compositor
  frame pacing), not in Mouser's code.
- Swappable GPU engines (t137u/t138u): design docs/engines.md (85cad5b). Trait
  `mouser-render/src/engine.rs` (`Engine`, `MouserEngine` wraps `gpu::GpuPainter`,
  `CustomLibrary`/`CustomEngine`, `FrameBuf` = DisplayList -> C frame, `load_installed`),
  `mouser-render/src/skia.rs` (feature `skia`, glutin + skia-safe, A8 glyph atlas,
  DontWait swaps), `mouser-core/src/engines.rs` (manifests, install/uninstall/select,
  `chosen()`, crash sentinel `engines/.starting-<name>`), `mouser-app/src/engine_cli.rs`,
  crates `mouser-engine-abi` and `mouser-engine-example`. Tests:
  `mouser-app/tests/engine_tests.rs` (builds the example into target/engine-tests),
  `mouser-render/tests/engine_frame_tests.rs`, unit tests in engines.rs and the ABI crate.
  Live (alien-blue, 1100x760 multi-image screen, drag-resize sweep): mouser 57 fps,
  skia 59–60, flat 59. Gaps: no hot swap, no URL install, full-frame damage only.
- Engine skip state (t140u): `engines::State`, `unskip`/`unskip_all`, `.failed-<name>`
  records, `engine::skip_check`/`fallback_message`/`Guarded` (crash marker for skia and
  custom), status-bar notice `NOTICE` in mouser-render/src/lib.rs; CLI `unskip`/`restore`.
- Still open: Fuzzball IME events and lock-key state; the known gaps; the `site` branch
  is not pushed yet (Pages not enabled); ratatui hyperlinks part 1 waits on upstream
  ratatui#2854.

## Open decisions
- Inside tmux, `mouser show` prefers Kitty placeholders over Fuzzball (one-line flip).

## Suggested next steps
0. Install the new package on the maintainer's machine (mouser-git from main or the next mouser-bin release) so command launching works there.
1. Add a LICENSE file and publish the AUR packages (maintainer).
2. Live-verify the remaining vttest menu 5 subtests and LNM; check Fuzzball physical key codes on a real keyboard.
3. Confirm website claims, retake the screenshot, publish the site.
4. Cut the next release once approved.
5. Fuzzball: IME events and lock-key state.
6. Work through the known gaps.
