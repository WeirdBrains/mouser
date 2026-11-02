# Mouser handover notes

Repository: https://github.com/WeirdBrains/mouser (moved from m3talsmith/mouser on 2026-10-31). Current release: v1.0.0 (2026-11-01). Homebrew formula/cask are updated automatically by the Release workflow. CI covers Linux, macOS and Windows.

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
  Edge cases left: the scrollback keeps colours and attributes (`sbstyle`, 1.0) but not OSC 8 links;
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
- Engine priority (t142u/t144u): `engines::{priority, saved_priority, normalize,
  set_priority, move_to, move_by}`, `Preferences::engines`; `engine::create(order)` walks
  it, `engine::after(order, failed)` for runtime fallback. mouser never skipped/removable.
- Engine switching + isolation (t141u): design docs/engines.md §7 (82675c5), host
  `mouser-render/src/engine_host.rs`, `Painter::{switch, fall_back}` and the
  `engine-prefs` watcher thread (UserEvent::Engines) in mouser-render/src/lib.rs,
  `engine::first_usable`, `engines::mark_skipped`. Tests
  `mouser-app/tests/engine_host_tests.rs` (pixels match, start failure, crash -> PTY shell
  alive, skipped, unskip). Switching itself is only live-tested (needs a window).
- Session/UI split + sidebar Engines (t145u): design docs/engines.md §8 (8ea37b6), code
  1e5e45e, 1af7013. `mouser-render/src/split.rs` (server `serve`, UI `ui_main`, headless
  test UI), `clipboard::set_remote`, `RenderHooks::{on_engine, on_window_size}`,
  `ui::{add_engines, EngineRow, EngineState}`, `UiAction::UseEngine`, app
  `engine_rows`/`use_engine`, `engines::clear_own_sentinels` and live-pid-aware
  `crashed_last_time`. Tests `mouser-app/tests/split_tests.rs`,
  `mouser-render/tests/sidebar_engines_tests.rs`. **Rollback point: annotated tag
  `pre-session-split` (adf349b)**, do not move or delete it. Off with `MOUSER_PROC_FORK=0` (was `MOUSER_SPLIT` before any release; no alias).
  Drag-to-reorder engines (t149u): `ui::EngineDrag` etc., drag state in `run`, drawn in
  `frame.rs`, `UiAction::ReorderEngines` -> app `reorder_engines` (`set_priority`).
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

## Modes Phase 1: commander (Oct 31, t153u–t164u)

State: Phase 1 complete (see "Phase 1 finished" below).

Decisions:
- Modes are stateful plugins: exactly one active (or none), switching replaces, no stacking. `mouser_render::modes::{Mode trait, Host}`; core has no commander branching.
- Commander is the only plugin and the default, active at startup.
- `esc :` + hotkey + Enter switches; the active mode's own hotkey turns modes off (assumption, MODES.md silent); `esc :c` again turns it on. Lone Esc waits 1200 ms (`[keys] leader_timeout_ms`; 500 ms before 1.0.1) then goes to the shell; `alt+:` opens the command line at once; nothing is intercepted on the alternate screen (view hidden too).
- View switch is in commander's own keymap: alt+t default (shadows readline transpose-words). All keys `<alt>+<key>`, remappable in `~/.mouser/modes.toml` `[commander.keys]`; unbound alt keys pass to the shell. ctrl+shift+k keymap window, ctrl+shift+m modes window.
- Previews lazy on a worker thread, only what is on screen; queue replaced as you move. Carousel: 1 full (focused) + 2 partial neighbours (cropped via src rect).
- Open: dir -> types `^U cd -- '<path>'`; video/audio -> xdg-open/open (not an in-app player, deviation); else full-width image/text view.
- cwd: /proc/<pid>/cwd, fallback OSC 7.

Files: crates/mouser-core/src/{modes.rs,commander.rs,lib.rs}; crates/mouser-render/src/modes/{mod.rs,commander.rs}, src/{lib.rs,ui.rs,frame.rs}, tests/commander_render_tests.rs, Cargo.toml; crates/mouser-app/src/{main.rs,tabs.rs}; Cargo.toml (image jpeg/gif/webp/bmp); docs/modes.md; CHANGELOG.md.

Next steps: confirm CI on all OSes; live-test keys in a test window (alt+t/s/arrows/enter, esc :c, ctrl+shift+k/m); build package (not installed) and give `sudo pacman -U` path; third-party mode loading (future, see docs/modes.md).

### Phase 1 finished (t167u/t168u) — STOP here, no later phase
- 8ee6963 navigation in every view (alt+arrows, PgUp/PgDn, Home/End, alt+p parent, alt+q/Esc close file view, click select / double-click open (400 ms) / wheel), multi-select (alt+space mark, alt+* all/clear, `*` + "N marked" shown), alt+| pipe prompt -> types `^U printf '%s\n' '<paths>' | <cmd>` (single-quoted, marked paths in listing order, else the selection); 1d683a5 Windows path fix in a test. Full table in docs/modes.md.
- Full suite, fmt, clippy green locally. Live (Hyprland focuswindow + verified activewindow, wtype): grid/carousel/pipe/file view/esc :c off-on all OK; screenshots ~/.cache/mouser-shots/live-*.png, mark.png. Note: wtype `-M alt space` types the word "space" — use `-k space`.
- Package: ~/.cache/mouser-pkg/mouser-git-0.7.2.r68.g1d683a5-1-x86_64.pkg.tar.zst (not installed).
- Open: in-app media player (uses xdg-open), third-party mode loading, modifiers other than alt.

### Typed line kept + media player (t170u/t171u)
- 7a8db14: cd/pipe keep the half-typed line: Ctrl+E, space, Ctrl+U (kill ring), command, Enter; when the shell is the PTY foreground again (tcgetpgrp == shell pid; Windows: short pause) Ctrl+Y + Backspace. Tested in a PTY with bash and zsh (tests/keep_line_tests.rs; fish not installed, test covers it if present); live on Michael's zsh. vi keymaps unsupported (documented).
- 0ddda31: in-app player (src/modes/player.rs): ffprobe for streams, `ffmpeg -re ... rawvideo rgba` frames through the image path fitted to width, `ffplay -nodisp -vn` for sound; alt+k play/pause, alt+j/l seek 10s, alt+-/= volume, alt+q/Esc close; keys pass to the shell when no player is open (Effect::Pass). Stops on close/selection change. Desktop opener only without ffmpeg. Live: frames change (player-video-1/2.png), ffplay running, 0 helpers after pause/close; audio view player-audio.png.
- Package: ~/.cache/mouser-pkg/mouser-git-0.7.2.r72.g0ddda31-1-x86_64.pkg.tar.zst. CI green on 0ddda31 (all 3 OSes).
- Open: A/V sync is by common start (no drift correction); pause/seek restart helpers (brief gap); no ffmpeg on CI macOS/Windows means the decode tests skip there. (superseded by a3a960d below)

- a3a960d: player rewritten: no ffplay; cpal audio sample clock (minus device latency) drives video frames by PTS (late dropped), wall clock without audio; pause/volume in-process; seek restarts both ffmpeg readers at -ss. Measured on Linux: A/V offset mean 20 ms (half a 25 fps frame), no drift over 66 s, seek gap ~80 ms. Needs alsa-lib (libasound2-dev on CI).

- 3f44c83 / c33f5cf: Phase 1 follow-ups. Player sync centred (mean -0.2 ms, p95 9.5 ms, 0 drops); back control (‹ back, alt+p, alt+Backspace, clickable path, no . / ..); video thumbnails (cached in $MOUSER_HOME/cache/thumbs); search feedback; popups opaque over previews; `[mouser]` line runs non-Mouser commands in the shell (`!` forces); double-Esc after closing a viewer no longer eats the next key; commander follows SSH sessions (OSC 7 + ssh side channel); carousel slides (180 ms ease-in-out).

### t181u/t182u: zoom, interactive player, rounded boxes + shadows, header ••• menu
- frame.rs: `round_rect`, `round_box`, `drop_shadow`, `RADIUS`=8; all panel styles, buttons, sidebar engine rows, modal, tabs; new `PanelStyle::Progress{done,hover}`.
- commander.rs: `Zoom`, `zoom_rect`, `zoom_about`, `header_fit`, `Hits`, `pointer_*` actions (`pack_xy`); lib.rs routes left press/move/release over the dock and ctrl+wheel (`zoom_wheel`).
- Not done: t185u remote ranged/parallel fetch with sparse cache + range map (and the separate fetch module for future sync, t186u). Remote files still show the "pipe it" notice.

### t183u-t186u: remote files
- mouser-core/src/fetch.rs: RangeMap, Remote (ssh side channel, tools), RemoteFile key, open()->Handle (prioritize, wait, read, url() HTTP range server, on_progress), backends rsync/scp/cat, LRU evict. Tests: range map, next() priority, HTTP parse, fake-ssh parallel fetch + priority + HTTP + head-only + LRU.
- commander (render): Previews has 3 workers + RemoteCtx; decode_remote (remote ffmpeg thumbs, early low-res, whole fetch); open_remote handle for viewer/player; heads for text tiles; fit = width. Player::open_input + keep.
- Host::set_remote(.., ssh); Mode::remote_ssh.
- measure: `MX_REMOTE_DIR=/tmp/mx/rmedia MX_SSH_DEST=fakehost MOUSER_SSH_CONFIG=/tmp/mx/sshd/ssh_config [MOUSER_SSH_BIN=/tmp/mx/slow-ssh MOUSER_REMOTE_BACKEND=cat] cargo test -p mouser-render --lib --release measure_remote -- --ignored --nocapture`.
- Live: inside the window, `ssh -F /tmp/mx/sshd/ssh_config -t fakehost bash --rcfile /tmp/mx/sshd/rc -i` (a plain ssh hits Michael's zsh/omz update prompt).
- Fixed: post-seek A/V catch-up spike on slow links (was up to ~570 ms): audio now holds after a seek until the first video frame is queued (`Shared::gate`, max 3 s). Open: first grid load sometimes slow to appear; sync not built.

### t198u: P1 plugin system (docs/plans/mcp-agent.md, docs/plugins.md)
- 8368b48: the plan records Michael's decisions (t196u-t197u): MCP + Fuzzball + command transports, no native libs in v1; MCP server stdio + loopback HTTP + HTTPS option (P2); llama.cpp built in + OpenAI/Anthropic-compatible URL; Gemma 4 E2B default with an E4B switch; any model is a `model-provider` plugin.
- a3c9f88 (core):
  - `plugins.rs`: the manifest, `$MOUSER_HOME/plugins/<name>`, `state.toml` (priority, disabled), and `Registry` with `providers(kind)` and `command(word)`. It lists commander and the engines as built-ins, and has `run_command` (JSON stdin/stdout, 10 s, scrubbed env).
  - `permissions.rs`: `Store`/`Broker` (allow/ask/never, `Scope` Installed / FuzzballLocal / FuzzballRemote; remote is never persisted).
  - `mcp.rs`: a hand-rolled stdio JSON-RPC client (initialize, tools/list, tools/call). rmcp was approved, but it is deferred to P3 to keep tokio out of P1.
  - `fuzzball.rs`: `p h`/`p q`/`p b`, `plugin_hellos`, `plugin_grants`, `sub=gkp`. Caps are joined with `+`, because `:` is the payload separator.
- d90247d (app):
  - `plugin_cli.rs` (`mouser plugin …`; an engine dir goes to the engine installer).
  - `approval_ui.rs`: a queue, 400 ms arming, and remote requests get only once/deny.
  - In main.rs, `plugin_hellos` is polled each layout from every tab. `resolve_approval` and `command_plugin`/`spawn_command_plugin` sit before the shell fallback on the `[mouser]` line. While a popup is open it takes keys, clicks and pastes.
  - In render: `ui::approval_panel`, `UiAction::Approve(u8)`.
- bcec94d: examples/plugins (hello-command sh, hello-mcp python3, hello-fuzzball sh), docs/plugins.md, the CHANGELOG.
- Tests: manifest/registry/priority/enable, command roundtrip + timeout + env scrub, the broker, the MCP fake server, Fuzzball `p` (and fuzzed), the CLI end to end with the examples, and popup layout/hit-test. CI green on bcec94d (all 3 OSes).
- Live (isolated temp HOME/MOUSER_HOME window; focus verified via `hyprctl dispatch 'hl.dsp.focus({ window = "address:0x…" })'`, since Hyprland 0.56 Lua needs the hl.dsp form; focus was given back to Michael's window after):
  - the Fuzzball popup, then `y`, gives `granted: notify+screen.read`;
  - F10 `hello Michael` shows the command popup, then `a` gives the notice "[hello-command] Hello, Michael!" and `run = "allow"` is saved.
  - Shots: ~/.cache/mouser-shots/p1-approval-fuzzball.png, p1-fuzzball-popup.png, p1-fuzzball-granted.png, p1-command-popup.png, p1-command-ran.png.
- Package (not installed): ~/.cache/mouser-pkg/mouser-git-0.7.2.r88.gbcec94d-1-x86_64.pkg.tar.zst.
- Open:
  - "a mouser command" is read as `[mouser]` line commands; Michael may clarify.
  - Granted capabilities are recorded, but nothing consumes them until P2/P3 (screen.read etc. become real with Mouser's MCP tools).
  - Built-ins can't be disabled yet (P6).
  - MCP plugins start only through `plugin probe` (P3 wires them to the sidebar and agent).
  - No approval UI on Windows for command plugins with sh (the examples need sh/python3).

### t200u: Allow always over ssh
- b83c7a7: `Scope::FuzzballRemote { host }`. The host is `SshTarget::identity()`: `ssh -G` gives user@hostname:port, and `ssh-keygen -l -F` on ssh's UserKnownHostsFile gives the SHA256 fingerprint. The key is `ssh:<id>/<plugin>` with one allow per capability, so extra capabilities, another host or another key ask again. Remote gets Allow once / Allow always / Deny (no Never); with no identity, once/deny only. `mouser plugin permissions` (no name) lists grants; `permissions '<key>' revoke` removes one. `ui::approval_panel` takes an `offer` list.
- Live: the test sshd (`sshd -f /tmp/mx/sshd/sshd_config`, port 22422, stopped afterwards); the window was started with `hl.dsp.exec_cmd(cmd, {float=true, no_initial_focus=true})` so it never steals focus. A script with `set -m` (so ssh is the tab's foreground) connects twice: the first shows the REMOTE popup, `a` is pressed, and the second is granted with no prompt. Shot: ~/.cache/mouser-shots/p1-ssh-sheet.png (popup | reconnect).
- CI green on b83c7a7 (3 OSes). Package: ~/.cache/mouser-pkg/mouser-git-0.7.2.r90.gb83c7a7-1-x86_64.pkg.tar.zst.
- Next: P2, Mouser's MCP server (stdio, loopback HTTP, HTTPS option), per docs/plans/mcp-agent.md.

### P2: Mouser's MCP server (docs/mcp.md)
- 7006d97 (core):
  - `mcp_server.rs`: JSON-RPC MCP over Streamable HTTP. It is written on std (no tokio; rmcp not needed), with JSON replies and no SSE; sessions are keyed by `Mcp-Session-Id`, so the server knows each client's name. Checks: Bearer token compared in constant time, Host/Origin loopback, 4 MiB body cap, 32 connections at most.
  - TLS is rustls with ring (no aws-lc, so no cmake). The self-signed cert is made by rcgen in `$MOUSER_HOME/mcp/{cert,key}.pem` (key 0600), or the user's PEM pair is used. `stdio_bridge` and the `post`/`client_tls` test client are here too.
  - `secrets.rs`: keyring 3 (Keychain / Credential Manager / Secret Service over zbus, so no libdbus). The account is `name@hash(MOUSER_HOME)`. Fallback is a `$MOUSER_HOME/secrets/<name>` 0600 file; `MOUSER_SECRET_STORE=file` forces it, and every test uses that.
  - `fileops.rs`: list, read, mkdir, copy, move, trash (to `$MOUSER_HOME/trash`).
  - `storage::McpServerPrefs` holds `[mcp.server]`.
- f872e56 (app):
  - `mcp_tools.rs`: `AppTools` with 11 tools. Reads need only the token, but a saved `never` still blocks them. Mutating tools call `ask()`: the broker key is `mcp:<client>`, a popup is shown with `Origin::Mcp{reply}`, and the call blocks up to 5 min. `start()` runs at launch when enabled.
  - `mcp_cli.rs`: `mouser mcp serve --stdio | status | enable [--https] | disable | token --show|--rotate`.
- Live (temp HOME/MOUSER_HOME, file secret store, ports 17801/17802; a stdlib Python MCP client covered all three transports):
  - initialize, tools/list (11), tabs_list and screen_read worked over stdio/HTTP/HTTPS; a bad token gets 401.
  - shell_run over HTTP showed the popup; `y` was sent with `hl.dsp.send_shortcut({key="y", window="address:…"})`, which needs **no focus**, and his active window stayed the same. MCP-WAS-HERE then appeared in screen_read.
  - files_mkdir over HTTPS showed the popup; `n` gave "the user denied it" and no folder was made.
  - The window was opened with `hl.dsp.exec_cmd(cmd, {float=true, no_initial_focus=true, ...})`, and it was checked that it was not focused.
  - Shots: ~/.cache/mouser-shots/p2-sheet.png (shell popup | ran | files popup).
- CI green on f872e56 (3 OSes). Package: ~/.cache/mouser-pkg/mouser-git-0.7.2.r93.gf872e56-1-x86_64.pkg.tar.zst.
- Open:
  - No SSE/streaming (JSON only) and no `notifications/*` from the server.
  - Not tried live: the real OS keychain. That was deliberate, to keep test entries out of his keychain; the CLI shows the store in use.
  - The server starts only at launch (restart after `enable`).
  - Client names are self-declared (the token is the trust boundary).
  - The stdio bridge needs the HTTP listener (it is always on when enabled).

### P3: MCP client and the sidebar MCP section (docs/mcp.md §Client)
- Commits:
  - 173566f core;
  - abc0e60 app;
  - f5f2cda docs.
- Package: `~/.cache/mouser-pkg/mouser-git-0.7.2.r97.gf5f2cda-1-x86_64.pkg.tar.zst` (not installed).
- Core:
  - `mcp.rs`: `Client` over stdio and Streamable HTTP(S) via `Client::connect(url, token, ca)`.
    - `Target::parse` refuses plain http to non-loopback hosts.
    - `post_to` tries every resolved address.
    - Replies may be JSON or SSE.
    - `ping` counts any reply as alive.
    - Public roots come from `webpki-roots`.
  - `mcp_client.rs`: `Manager` with a thread per server.
    - States: Connecting, Connected, Failed(reason, retry), NeedsApproval, Denied, Disabled.
    - Backoff from 1 s to 5 min; ping every 30 s.
    - Sources are `[[mcp.servers]]` plus enabled MCP plugins (`plugin:<name>`).
    - `broker_name` gives `mcp-server:<name>` or the plugin name; `tool_cap` gives `tool.<name>`.
  - `storage::McpServerEntry`: name, command, env, url, token (`keychain:NAME`), ca, enabled.
- App (main.rs):
  - `mcp_clients()` (OnceLock), `mcp_rows`, `mcp_server_click`, `mcp_tool_click`/`mcp_call`, `spawn_mcp_call` and `mcp_command` (the `[mouser]` line: `mcp`, `mcp tools|reconnect ID`, `mcp call ID TOOL [JSON]`).
  - New approval origins: `McpRun` and `McpTool`.
  - The secret for each server's token is named `mcp-server-NAME`, with no colon so the file fallback is Windows-safe.
  - `mouser plugin permissions mcp-server:NAME tool.X=allow` works.
- Render: `ui::add_mcp` runs before `add_engines`; the McpRow kinds are Title, Server, Tool and Note. Clicking a connected server toggles its tool list.
- Live check:
  - Temp MOUSER_HOME/HOME, `SHELL=/bin/sh`, file secret store, ports 17801/17802.
  - The window was floating and unfocused (verified), and keys were sent with send_shortcut.
  - self over http and self-https (with `--ca`) were connected to Mouser's own P2 server with 11 tools each; hello (stdio) and plugin:hello-mcp (after the `run` popup) were connected too.
  - tabs_list and `greet {"name":"Michael"}` ran through the broker popup.
  - Screenshots: `~/.cache/mouser-shots/p3-b.png` and `p3-sheet.png` / `p3-combo.png`.
- Deferred: resources, prompts, list-changed, OAuth, project-local config, sidebar enable toggles, and live reload of prefs (applies on the next start).
- Next: P4 (model providers). It needs Michael's go-ahead, and it hits the hard stops (llama.cpp deps, model downloads).

### P4: model providers as plugins (docs/models.md)
- Commits:
  - a350df0 core;
  - 3c891ca app;
  - 9b4089f docs.
- CI: run 41342521462, green on Linux, macOS and Windows.
- Package: `~/.cache/mouser-pkg/mouser-git-0.7.2.r101.g9b4089f-1-x86_64.pkg.tar.zst` (not installed).
- Core (`crates/mouser-core/src/models/`):
  - `mod.rs`: `ModelProvider`, `ChatRequest`/`Message`/`Part`/`ToolSpec`/`ToolChoice`, `Event` (Text, ToolStart, ToolArgs, Usage, Done), `ChatSink` with `cancelled()`, the `Collect` sink, `PRESETS`, `[models]` prefs and the registry (`all`/`find`/`open`).
  - `http.rs`: a std + rustls client with chunked and SSE line streaming. Plain http goes to loopback only.
  - Adapters:
    - `openai.rs`: Chat Completions, including the Kimi K2 quirks.
    - `anthropic.rs`: Messages, version 2023-06-01.
    - `google.rs`: `streamGenerateContent?alt=sse`.
  - Plugins: the `model-provider` kind gets the `http` transport plus a `[model]` table. BUILTINS has 8 entries (six presets, gemma-local and the others).
  - Keys are `keychain:model-NAME` refs; preferences.toml never holds a key.
- App: `model_cli.rs` provides `mouser model list | add | remove | test | default`. `add` takes `--key-stdin`, `--api`, `--url`, `--model` and `--ca`.
- Tests: mock HTTP servers only (streaming, tool deltas, chunked bodies, errors, cancel, Kimi quirks, registry). A live binary smoke test ran against a Python mock for all three adapters. No real keys were used.
- Deferred: the Responses API, images, retries, `/models` listing and the plugin `network` grant.

### P4b: the built-in local model (docs/models.md §The local model)
- Commits:
  - 143858b core;
  - c61fc45 app + mouser-llm;
  - 37cb9d8 docs/PKGBUILD;
  - then a macOS test-race fix and the variant deps (see git log).
- Packages (not installed):
  - default: `~/.cache/mouser-pkg/mouser-git-0.7.2.r105.g37cb9d8-1-x86_64.pkg.tar.zst` (29 MB binary);
  - local-llm: `~/.cache/mouser-pkg/llm/mouser-git-0.7.2.r105.g37cb9d8-1-x86_64.pkg.tar.zst` (73 MB binary, links libvulkan.so.1, so the variant `depends` on vulkan-icd-loader).
- Core (`models/local.rs`):
  - The weights catalog has E2B and E4B Q4_0 from ggml-org, with pinned sizes and SHA256.
  - `dir()`: `MOUSER_MODELS_DIR`, then `[models.local] dir`, then `$MOUSER_HOME/models`.
  - `download(Consent, url, progress)` resumes the `.part`, verifies size and SHA256, and needs a `Consent` value.
  - `LocalBackend` registry, `LocalPrefs`, and the `Local` provider: a url delegates to the openai/anthropic adapter; otherwise the weights and the backend are required.
  - `render()` runs the GGUF Jinja template through minijinja (with pycompat); `render_fallback()` produces the Gemma format.
  - Tools: `tool_prompt` plus the `ToolParser` for `<tool_call>` tags.
  - `http.rs` gained `request`/`get` with redirects.
- `crates/mouser-llm`: llama-cpp-2 0.1.160 (Vulkan; Metal on macOS) implementing `LocalBackend`.
  - The model stays cached, and all layers go on the GPU by default.
  - Context 8192, prompt batches of 512.
  - Sampling: top_k 64, top_p 0.95, temp 1.0, or greedy at temperature 0.
  - Output is UTF-8 safe and streams through the ToolParser, with cancel checks. `MOUSER_LLM_TIMINGS=1` prints speeds.
  - The crate is in the workspace `exclude` list, so `--workspace` builds and CI never compile llama.cpp. mouser-app's feature `local-llm` = `dep:mouser-llm`, and main registers it at startup.
- App:
  - `mouser model local [status] | download [ID] [--yes] | delete ID | url URL [--api] [--model] [--key-stdin] / --clear | dir PATH / --clear`.
  - `[mouser] model download [ID]` opens a once-only approval popup (`approval_ui::Request::once`, never passed to the broker) and then downloads in the background with 10% notices.
  - `mouser model test --tool` does a full round trip with a demo tool result.
- Packaging: `MOUSER_LOCAL_LLM=1 makepkg` adds `--features local-llm`. Default deps are unchanged; the variant also needs cmake, clang, vulkan-headers, shaderc and spirv-headers.
- No CI job for the feature, because it needs Vulkan headers and shaderc that the runners don't have. Instead it was build-checked locally: release build plus `clippy --features local-llm -D warnings`.
- Live check (Michael OK'd the E2B download, t206u):
  - The weights were downloaded with Mouser's own downloader into `/mnt/ext1/home/m3talsmith/Projects/.models/gemma/` and verified against the published SHA256.
  - Run with a temp HOME/MOUSER_HOME and `MOUSER_MODELS_DIR` pointing there.
  - RTX 3060 via Vulkan: load about 1.0 s warm, prompt 2,700-5,400 t/s, generation 91-119 t/s.
  - VRAM: 1,227 MiB baseline → 3,396 MiB peak, so about 2.2 GB for E2B plus an 8k context.
  - The file was re-verified with sha256sum against the HF API LFS oid.
  - The `get_time {"city":"Oslo"}` call → demo result → "The current time in Oslo is 14:05."
  - The first long prompt is slow once, while the NVIDIA driver compiles shaders.
  - In-app popup: screenshots `~/.cache/mouser-shots/p4b-popup.png` and `p4b-denied.png`. Deny downloads nothing and saves no permission.
- Deferred: E4B not downloaded; no GPU selection or KV-cache reuse across turns; the popup can't cancel a running download; chat mode is P5.

### P5: chat mode, the agent (docs/chat.md)
- Commits:
  - 42ec7b0 core;
  - bcae534 render;
  - 15b2524 app;
  - e737bed docs.
- CI: run 41351536334, green on Linux, macOS and Windows.
- Packages (not installed), in `/mnt/ext1/home/m3talsmith/Projects/.cache/mouser-pkg/`. Big build dirs now live on /mnt/ext1, not /tmp or ~/.cache.
  - default: `mouser-git-0.7.2.r110.ge737bed-1-x86_64.pkg.tar.zst`;
  - local-llm: `llm/mouser-git-0.7.2.r110.ge737bed-1-x86_64.pkg.tar.zst` (depends on vulkan-icd-loader).
- Core: `agent.rs`.
  - `Chat` (send/cancel/answer/clear, `State` with entries and the pending card) and the `AgentHost` trait.
  - `Check` (Allow / Deny / Ask lines) and `Answer` (Once / ForChat / Deny).
  - `redact()`, `untrusted()`, MAX_STEPS 8, tool output capped at 8,000 characters, transcripts `$MOUSER_HOME/chats/*.jsonl` (0600).
  - `Preferences.chat` (`[chat] transcripts`).
  - Malformed calls with an empty name are dropped; small models sometimes emit them.
- Render: `modes/chat.rs` `ChatMode` (hotkey `a`, the bindings, the dock with header, transcript, card and input) and `Host::push`.
- App:
  - `chat_host.rs` `ChatHost`. Mouser tools run through `AppTools { preapproved: true }` after the chat card. Mutating tools always ask; reads obey only a broker "never" for `chat`. MCP tools follow `broker_name`/`tool_cap`.
  - Registered after `mcp_tools::start` in main.
  - `examples/chat_live.rs` (required feature local-llm).
- Tests: `agent_tests` (8: allow, card once/deny/never, for-chat vs always-ask, cancel mid-stream/card/tool (the tool sees the cancel), step limit, transcript, redaction, nameless call) and `chat_render_tests` (2).
- Live check:
  - Temp HOME/MOUSER_HOME, `SHELL=/bin/sh`, scratch in `/mnt/ext1/home/m3talsmith/Projects/.cache/mouser-p5`.
  - Mock provider (loopback, port 17901) in a floating, unfocused window:
    - redacted token (`p5-1-mock-redacted.png`);
    - the `shell_run` card (`p5-2-card.png`), allowed and ran in the tab (`p5-3-allowed.png`);
    - Esc cancel mid-stream (`p5-4-cancelled.png`);
    - deny (`p5-5-denied.png`), all in `~/.cache/mouser-shots/`.
  - Gemma 4 E2B via Vulkan, headless `chat_live` with a sandboxed sh:
    - `ls -la` card → allow → output → correct file list;
    - the token question was answered with `DEMO_TOKEN=[redacted]`;
    - warm: prompt 5,000–8,900 t/s, generation 75–110 t/s, a turn takes 1.4–2.4 s (10.8 s when a new prompt size compiles shaders).
  - The GUI Gemma screenshot was skipped: the test window was moved to another workspace mid-test, so a region capture would only show the user's screen.
- Open: the Edit button on the card, a model picker in the dock, cost, images, a light-theme screenshot, and open questions 12–14 (answered with the plan's proposals).

### P6: modes and engines are plugins (docs/plugins.md)
- Commits:
  - 7975623 core;
  - 33c424e render/app/CLI + example;
  - 1ff252f docs;
  - 3be0167 queued-event cwd fix;
  - 4ee3e76 Windows clippy fix.
- CI:
  - run 41356839191 failed on Windows clippy (unix-only test helpers), fixed in 4ee3e76;
  - run 41357399586 green on Linux, macOS and Windows.
- Packages (not installed), in `/mnt/ext1/home/m3talsmith/Projects/.cache/mouser-pkg/`:
  - default `mouser-git-0.7.2.r116.g4ee3e76-1-x86_64.pkg.tar.zst`;
  - local-llm `llm/mouser-git-0.7.2.r116.g4ee3e76-1-x86_64.pkg.tar.zst`.
- Core (`plugins.rs`):
  - compiled-in manifests `src/builtin/{commander,chat,mouser,skia}.toml` (`BUILTIN_MANIFESTS`, `builtin_manifest()`);
  - `[engine]` (transport `library`) and `[mode]` (hotkey, `[[mode.keys]]`) tables;
  - `Manifest::from_engine` / `to_engine`: engine.toml read as plugin.toml, and `engines::read_dir_manifest` prefers plugin.toml;
  - `set_enabled` works for built-ins and engines except `mouser` (`can_disable`);
  - `Registry::modes()` / `engines()`, `Entry.hotkey`;
  - `CommandReply` gains `lines` and `title`;
  - installing a mode keeps the built-in modes ahead of it.
- Core (`engines.rs`):
  - `priority()` drops disabled engines (they keep their place in prefs);
  - `select` refuses disabled ones;
  - `MOUSER_ENGINE` is unfiltered.
- The engine order stays in the `engines` preference, not `plugins.order.engine`; see plan §8 "As built".
- Render:
  - `Host::from_registry(&Registry, make)`: priority order; hotkey clashes are left out with a note; none enabled gives a plain terminal;
  - `Host::names()`;
  - `modes/plugin.rs` `PluginMode`: the command transport, one background run per event (activate, key, cwd; latest queued). `run` must be allowed, and `files.read` is needed for the cwd.
- App:
  - main builds the mode host from the registry where chat used to be pushed; notes are shown after that;
  - the sidebar engine list comes from `Registry::engines()`;
  - one clap enum serves `plugin` and `engine` (`plugin_action`), with `--kind` global on `plugin`;
  - `plugin_cli::run_kind` routes kind engine to `engine_cli`;
  - `engine_cli::list` is registry-driven and shows disabled engines.
- Example: `examples/plugins/hello-mode` (mode on `h`, keys t and f).
- Tests:
  - core `builtin_manifests_are_plugin_toml`, `mode_and_engine_manifest_checks`, `engines_are_plugins_disable_and_migrate` (old `engine = "skia"` key);
  - render `mode_registry_tests`;
  - app `engines_and_modes_through_the_plugin_cli` (engine list == plugin list --kind engine) and `plugin_toml_engine_installs_through_the_plugin_cli` (real flat engine);
  - main.rs parse test: engine X == plugin X --kind engine;
  - existing commander and engine tests unchanged.
  - Full workspace: 423 passed, 0 failed, 4 ignored.
- Live check (release build, temp HOME/MOUSER_HOME, `SHELL=/bin/sh`, scratch `/mnt/ext1/home/m3talsmith/Projects/.cache/mouser-p6`):
  - in the CLI: engine list == plugin list --kind engine (diff), `disable mouser` refused, `use` of a disabled skia refused;
  - in a floating, unfocused window: `esc :h` switched to hello-mode, and its program logged `activate`, `cwd`, `key time`, `key files` with the cwd; switched to `:a` and `:c`.
  - No P6 screenshots: the test window is translucent, so a region capture shows what is behind it. Captures that did were deleted at once. Next time make the test window opaque first, or capture headless.
- Open:
  - MCP-transport modes (rejected with a message);
  - mode plugins can't type into the shell or read the screen;
  - the skia row is listed even when not in the order (as before);
  - questions 15–16 (engine alias kept; skia stays in the package);
  - the full test suite's deliberate engine-crash tests leave SIGABRT core dumps of the debug binary, and the desktop shows a "Process crashed: mouser-app" notice.
