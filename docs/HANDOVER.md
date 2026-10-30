# Mouser handover notes

Current release: v0.7.2. Homebrew formula/cask are updated automatically by the Release workflow. CI covers Linux, macOS and Windows.

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
| terminfo | `tic -x -o ~/.terminfo extra/mouser.terminfo`, then `TERM=mouser` | `infocmp mouser` |

Verified live (wtype, 2026-10-30): Fuzzball key events, Kitty keyboard flags 1/9/27/31 and `CSI ? u`, vttest menu 5 cursor keys (normal + application) and function keys F1–F12 (F10 is Mouser's command line, so it never reaches the app; F13+ send nothing in legacy mode). wtype uses a virtual keymap, so `code=` reads `Escape` and Shift does not change the character; physical codes need a real keyboard.
Not yet verified live: connection click opening a
tab, vttest menu 5, and the cause of vttest's LNM "Not expected".

## Known bugs
- (Fixed in 0.7.1: dashed box-drawing lines, now drawn procedurally with one shared integer geometry (light, heavy and double lines) in `crates/mouser-render/src/boxdraw.rs`, identical on CPU and GPU; the "81 wide" vttest report was the window really being 81 columns, covered by a regression test.)
- Website install code blocks clip long commands (scroll sideways).

## Known gaps
DECSLRM · colour emoji · real italic face (synthetic now) · visible blink ·
DECCOLM 132-column · G2/G3/national charsets · protected areas · ENQ answerback ·
OSC 133 UI.

## Fuzzball
Spec v0.1 with resolved decisions in `docs/protocol/README.md` §10. Kitty graphics, OSC 1337
and xterm-kitty were removed; Kitty keyboard is the only fallback.
Not yet done: placement removal when rows leave scrollback, reflow movement, IME events,
lock-key state, upload timeout only checked on the next chunk.

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

## Suggested next steps
1. Live-verify Kitty keyboard and Fuzzball key events, connection-click tabs, vttest menu 5, LNM.
2. Confirm website claims, retake the screenshot, publish the site.
3. Cut the next release once approved.
4. Finish the Fuzzball gaps listed above.
5. Work through the known gaps.
