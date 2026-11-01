## [Unreleased]

### Added (t183u-t186u): remote files like local ones

- Commander in SSH sessions shows thumbnails, opens the viewer (zoom included) and plays video/audio with seek, instead of a notice.
- New `mouser_core::fetch` layer (reusable for a later sync): ranged, parallel reads (3 segments per file, 3 files at once) over the session's ControlMaster side channel into a sparse cache (`$MOUSER_HOME/cache/remote`, key host+path+size+mtime, filled-range map, LRU cap `MOUSER_REMOTE_CACHE_MB`, default 2048). The first read is the 256 KB head; seeks and scrolls prioritise their range; off-screen / closed transfers stop, small ones (`MOUSER_REMOTE_COMPLETE_MB`, default 64) finish into the cache with rsync (both ends) or scp, else ranged reads.
- Remote video plays through a local HTTP range server backed by the fetch layer (ffmpeg seeks become prioritised reads); thumbnails come from the host's ffmpeg when it has one, else a ranged read decoded locally.

### Changed

- The image viewer's fit (default and alt+0) fits the WIDTH; tall images scroll/pan.

### Added (t181u/t182u)

- Commander image view zoom: alt+= / alt+- (same keys as player volume; each view takes its own), alt+0 fit, alt+1 100%; ctrl+wheel zooms about the pointer; pan by drag, alt+arrows or the wheel; zoom % in the header; full-resolution decode past the 2048 px preview; clipped to the view.
- Interactive player: clickable « 10s / play-pause / 10s » buttons, a seek bar (click seeks, drag scrubs, hover shows the time), a volume bar (click, drag or wheel). Video and audio.
- Every box (panels, popups, options/key/mode lists, search field, buttons, tiles, sidebar rows, tabs) has 8 px rounded corners and a soft, theme-aware drop shadow (unified with the old popup shadow). Drawn from plain rects, so all renderers (wgpu, skia, CPU, engine plugins) get it.
- Narrow commander header: buttons that do not fit (after the path shortens) collapse progressively into a ••• menu at the upper right (click or alt+i); rounded, shadowed dropdown; Esc closes it.

Fixed:
- Player sync: smooth audio clock between callbacks and a half-redraw lead; measured mean -0.2 ms, p95 9.5 ms, 0 drops (was +20 ms, 6 % dropped)
- The first key after closing a viewer with a double Esc was eaten (Esc glued to it as meta-key)
- Options window: previews no longer show through it; popups get a drop shadow
- Viewer images and player frames are clipped to the view (never over the prompt)
- Player: alt+p (back) closes the player instead of changing folder under it

Added:
- Mode plugins and the commander mode (MODES.md Phase 1, docs/modes.md): one mode at a time, commander on at start; `esc :c` (Enter) switches modes / turns the current one off (a lone Esc reaches the shell after 500 ms; nothing is taken in full-screen apps); `ctrl+shift+k` key mappings, `ctrl+shift+m` modes; keys remappable in `~/.mouser/modes.toml`. Commander docks a listing of the shell's directory above the prompt as a list (`ls -lah` columns), grid (previews by type, tile sizes) or carousel (focused item plus two partial neighbours), with search, an options window (view, `-a`, `-h`, sort, reverse), a manual, and open (cd into folders, desktop opener for video/audio, full-width image/text view); previews load lazily for what is on screen only. Navigation in every view: alt+arrows, PgUp/PgDn, Home/End, alt+p parent folder, alt+q / Esc closes the file view, click selects, double-click opens, wheel moves; multi-select (alt+space mark, alt+* all) and alt+| pipe (types `printf '%s\n' <quoted paths> | <cmd>` into the shell). Modes implement the `mouser_render::modes::Mode` trait; loading third-party modes is future work
- Commander's media player: video (ffmpeg frames through the image path, fitted to the width) and audio (played in-process through cpal; the audio device's sample clock drives the video frames by PTS, late frames dropped; pause, volume and seek without restarts except a seek re-starting both ffmpeg readers at the target) with play/pause, seek, volume and close keys (alt+k, alt+j/l, alt+-/=, alt+q); stops when the viewer closes or the selection changes; the desktop opener is only the fallback without ffmpeg
- Commander follows SSH sessions: remote folder from OSC 7, listed over a multiplexed BatchMode ssh side channel, `user@host:/path` header, remote badges, clear "no directory info" state
- Commander carousel slides between items (180 ms ease-in-out, retargets mid-slide, redraws only while moving)
- Commander: back control in the header (`‹ back`, alt+p, alt+Backspace; clickable path segments; disabled at /); `.` and `..` are no longer listed; going up selects the folder you came from
- Commander: video thumbnails in grid and carousel (ffmpeg frame at 10 %, cached in `$MOUSER_HOME/cache/thumbs`), quieter placeholders, focused carousel item loads first
- Commander: search shows the query, the match count or "no matches", and its focus
- `[mouser]` command line: a line that is not a Mouser command runs in the tab's shell (`!` forces the shell)
- Commander's cd and pipe keep what you have typed (cut to the kill ring and yanked back when the prompt returns; bash, zsh, fish in emacs mode)
- Image previews read JPEG, GIF, WebP and BMP as well as PNG
- Session server / UI split (Linux, macOS): the window and its engine run in a child `mouser __ui` process; sessions, PTYs, scrollback, tabs and images stay in the process you started. A native crash in any engine (built-in too) restarts the window on the next engine in the priority order with everything kept; the crashed engine is marked skipped with the signal; a crash in `mouser` restarts on the CPU renderer; a UI killed from outside restarts on the same engine. `MOUSER_PROC_FORK=0` keeps one process; Windows is unchanged (docs/engines.md §8)
- Sidebar Engines section: available engines in priority order with active/ok/skipped/failed and the reason; a click switches the window live (same as `mouser engine use`); drag rows to reorder (insertion line and ghost; Escape or a drop outside cancels; saved like `mouser engine priority` and applied live)
- Test hooks `MOUSER_TEST_CRASH_ENGINE=<engine>:<n>` and the headless test UI (`MOUSER_UI=headless`); `key-to-frame` timing in `RUST_LOG=mouser_frame=debug`
- Live engine switching: running windows swap engine in place when the order or a skip mark changes (sessions, PTYs, scrollback, selection and images untouched)
- Crash isolation: custom engines run in an engine host process (`mouser __engine-host`); a crash or hang there marks the engine skipped with the reason and the window continues on the next engine; panics in built-in engines are caught the same way; ABI adds `MOUSER_SURFACE_PIXELS`/`MouserPixels` (additive, still ABI 1); example engine option `MOUSER_FLAT_CRASH_AT`
- Engine priority order: `engines = [...]` in preferences.toml (old `engine` key still read), `mouser engine priority a b ...`, `mouser engine move <name> first|last|up|down|<n>`, `use` moves an engine first; startup and runtime failures walk the order to the next engine; `MOUSER_ENGINE=a,b` overrides it for one run; `mouser` is always in the order, cannot be removed, uninstalled or skipped, and stays the final GPU fallback before the CPU renderer
- `mouser engine list` shows each engine's state (ok, skipped after a crash with when, failed with when and why); `mouser engine unskip <name>` / `--all` (alias `restore`) clears it without selecting; fallbacks print, log and show in the status bar which engine was skipped and the command to restore it; `skia` is crash-guarded like custom engines
- Swappable GPU rendering engines: `mouser` (the wgpu renderer, default), `skia` (Skia over OpenGL; in default builds, `--no-default-features` drops it) and custom engines, shared libraries with a versioned C ABI (`mouser-engine-abi`, `mouser_engine.h`); `mouser engine list | install <dir> | uninstall <name> | use <name>`, `MOUSER_ENGINE` overrides; engines live in `~/.mouser/engines/`; an engine that fails to load, reports another ABI, errors while drawing or crashed the last start falls back to `mouser`; example engine `crates/mouser-engine-example` (`flat`); the CPU fallback is unchanged (docs/engines.md)
- Re-wrap on resize: soft-wrapped lines on the main screen and in the scrollback are joined and split at the new width (hard breaks kept, wide characters never split, attributes, links and combining marks kept); the cursor stays on its character; images stay anchored to their line, and fitted images take or give back the empty rows below them so text after them moves instead of being covered; the alternate screen is cut, not re-wrapped, and the main screen behind it is now kept (and re-wrapped) instead of blanked; a resize clears the selection
- Smooth drag-resize with any scrollback: each resize re-wraps the screen and the newest 2000 scrollback rows (about 1 ms); older lines follow on a worker in small steps; idle scrollback compression does one chunk at a time instead of stalling a frame; frame and resize timings with `RUST_LOG=mouser_frame=debug`
- Tests for several images at once (fitted, fixed, `--original`, Kitty `c`/`r`, in the scrollback): no placement overlaps another or covers text after any resize; shrinking an image never removes a line another image or the cursor is on
- Fuzzball capability reply advertises `g_fit=w` and `g_reflow=1`; `mouser show` sends `fit=w` only when `g_fit` lists `w`
- Mouse selection: drag selects characters, double-click a word (`[selection] word_chars` in config.toml), triple-click a line, Shift+click extends, Alt+drag selects a block; works over the scrollback, auto-scrolls past the window edges, handles wide and combining characters, joins soft-wrapped lines and trims trailing blanks; Shift bypasses an app's mouse reporting; cleared when its text changes or a typed key is sent
- Copy with Ctrl+Shift+C (Cmd+C on macOS); selecting sets PRIMARY on Linux (through the Wayland connection, so it works on Hyprland)
- Paste: Ctrl+Shift+V (Cmd+V on macOS) pastes the clipboard, middle-click and Shift+Insert paste PRIMARY on Linux; pasted control characters (except tab, newline, CR) are dropped and a pasted `ESC[201~` cannot end a bracketed paste early
- Themes can set `selection_foreground` / `selection_background` (inverse colours otherwise)
- Arch packaging: AUR-ready `mouser-git` (source) and `mouser-bin` (release binary) PKGBUILDs with `.SRCINFO` in `packaging/arch/`; both install the desktop entry, hicolor icons, metainfo and the compiled terminfo, and replace each other and older `mouser-bin` installs; the release workflow updates the `mouser-bin` checksums after publishing
- Run a command: `mouser -e CMD ARGS...` and `mouser -- CMD ARGS...`, plus `--working-directory`/`--dir`, `--title`/`-T`, `--class`/`--app-id`, `--hold` and `--version`; the desktop entry declares them for xdg-terminal-exec (`X-TerminalArg*`), so Mouser works as the default terminal on Omarchy
- Fuzzball protocol v0.1 (`docs/protocol/`): image upload/place/delete/query, keyboard mode with structured key events, capability query (`DCS = F z c q`, XTGETTCAP `Fz`)
- Golden-pixel tests for Fuzzball images (CPU and the GPU display list)
- `tools/fuzzball-demo.py` demo client
- `mouser show <file|->...`: show PNG, JPEG, GIF (first frame), WebP and BMP images in a Fuzzball terminal, fitted to the width (`--width`, `--height`, `--original`, `--id`); no video or audio
- Fuzzball placements follow their text through scroll regions, SU/SD, IL/DL, the scrollback and resize; they are removed when their anchor row leaves the scrollback, scrolls off the alternate screen
- Fuzzball images scale to fit the screen width on resize (and when placed) and grow back on a wider screen; downscaled images are area-averaged and cached for both renderers; cell-size changes rescale their pixels
- Abandoned Fuzzball uploads are discarded by a timer after 10 s
- ED 3 (`CSI 3 J`) clears the scrollback (and the placements in it) without touching the screen
- Kitty graphics protocol as a fallback behind Fuzzball (graphics.md §7): direct/chunked, file, temp-file and shared-memory transmission, `f=8/16/24/32/100`, zlib, every action and delete specifier, replies with `q`, feature queries, image numbers, relative placements, Unicode placeholders, animation and frame composition, z layers under/over cell backgrounds; shares the placement lifecycle, renderer, scale cache and memory quota with Fuzzball
- `mouser show` falls back to Kitty graphics when Fuzzball does not answer, and works inside tmux (`allow-passthrough on`) through Kitty Unicode placeholders

Changed:
- A start marker left by another window that is still starting is no longer taken as a crash (two windows starting at once could skip each other's engine)
- Images always fit the terminal width: `mouser show` scales each image up or down to the full width (starting on a new line), and Fuzzball placements made with the new `fit=w` key follow the width on every resize; `--width`, `--height` and `--original` still give fixed sizes
- A placement that would overhang the right edge moves to column 0 at its requested size (scaled down only when wider than the screen) instead of shrinking in place and pinning to the last column at one column wide; Kitty placements with explicit `c`/`r` keep their size whenever it fits
- Kitty keyboard protocol kept as a fallback behind Fuzzball keyboard mode
- Kitty keyboard: Ctrl/Alt/Super keys carry no associated text (flag 16)
- Kitty keyboard compliance fixes: F13+/media/keypad keys, lock bits on text keys, Enter/Tab/Backspace releases, Ctrl+Backspace, Ctrl+Shift/Super text keys in legacy mode; tests per spec section

Removed:
- `OSC 1337` image claims from the docs (never implemented)
- `xterm-kitty` TERM fallback; TERM is `mouser` or `xterm-256color`

## [0.7.2] - 2026-10-30

Fixes:
- Box-drawing lines align identically on the CPU and GPU renderers
- Double-line box characters drawn procedurally to match single lines
- Windows CI: glyph snapshot tests ignore CRLF line endings

Docs:
- Handover notes updated

## [0.7.1] - 2026-10-30

Added:
- Tabs; clicking a connection opens it in a new tab
- GPU renderer (wgpu) with CPU fallback (`MOUSER_RENDERER=cpu`)
- Fuzzy font picker listing installed fonts
- Sidebar remembers its open/closed state; starts collapsed by default
- Kitty keyboard and graphics protocols
- Fuzzball protocol draft spec (`docs/protocol/`)
- Broader xterm/ECMA-48 support: SGR styles, wide chars, mouse, focus, bracketed paste, OSC titles, links and clipboard, window reports
- Public handover notes (`docs/HANDOVER.md`)

Changed:
- Event-driven redraws and glyph cache: near-zero idle CPU

Fixes:
- Window closes when the shell exits (Ctrl+D)
- Font editor no longer freezes while typing
- vttest fixes: DEC line drawing, SI/SO, DECSCNM, LNM, DECREQTPARM, DA1
- Box-drawing lines are continuous (no dashed edges)

## [0.7.0] - 2026-10-29

Feature: UI scaffolding for connections
- Real sidebar/modal layout engine with hit-testing
- Render tests for sidebar, modal, and accordion toggle
- Basic wiring in app to show sidebar and open modals

## [0.6.8] - 2026-10-29

Fixes:
- Release (Linux): move Arch .pkg.tar.zst out of container-mounted work dir using sudo to avoid permission errors.

## [0.6.7] - 2026-10-29

Fixes:
- Arch packaging in Release workflow: build without sudo by preinstalling deps and using `makepkg --nodeps`.
- Icon: small-size SVG uses exact Red Star background gradient; regenerated PNG/ICO/ICNS/hicolor assets.

## [0.4.2] - 2026-10-29

## [0.6.6] - 2026-10-29

- Packaging: Arch build installs runtime deps via makepkg -s.


## [0.6.5] - 2026-10-29

- Packaging: fix hicolor tree path and Windows installer source path.


## [0.6.4] - 2026-10-29

- Release: fix sed token replacement for nfpm under strict shell.


## [0.6.3] - 2026-10-29

- Release: fix nfpm env expansion; correct Windows Inno Setup paths.


## [0.6.2] - 2026-10-29

- Icon: further refine mouse silhouette for clarity at 32–64px.
- CI: remove runtime icon generation; use committed assets.


## [0.6.1] - 2026-10-29

- Release: fix Windows installer icon/output paths; use committed hicolor icons for Arch.


## [0.6.0] - 2026-10-29

- Icon: redesigned Red Star mouse silhouette; integrated >_ prompt; full palette.
- Theme: add Red Star (dark/light) as default; docs updated.
- Packaging: deb/rpm via nfpm, Arch mouser-bin, macOS DMG + Homebrew formula & cask, Windows Inno Setup installer; desktop launcher & icons.


Fix: cross-platform config/home resolution and Windows build
- Remove `xdg` usage in app config loader; use core `resolve_home()` (respects `MOUSER_HOME`)
- Unblock Windows build/clippy by avoiding Linux-only XDG calls
- Quality bar: fmt/clippy/tests green locally; CI expected to pass

## [0.4.3] - 2026-10-29

Fix: Release workflow stability across platforms
- Install Rust targets per matrix to fix macOS cross build (add `targets` to toolchain)
- Windows checksums via PowerShell `Get-FileHash` (no `shasum` on Windows)
- Drop Linux aarch64 artifact for now (needs cross-linker); keep x86_64

## [0.4.4] - 2026-10-29

Fix: Stabilize macOS CI tests
- Serialize theme tests to avoid `MOUSER_HOME` environment races on macOS runners

## [0.4.5] - 2026-10-29

Fix: Windows release checksum
- Correct PowerShell quoting for writing `*.sha256` (previously invalid path)

## [0.4.6] - 2026-10-29

Fix: Windows release checksum path
- Use absolute path (`Join-Path $PWD $Env:ARTIFACT`) for `Get-FileHash` and output

## [0.4.7] - 2026-10-29

Fix: Windows release checksum path (final)
- Use `$Env:GITHUB_WORKSPACE` with `Join-Path` for stable absolute path

## [0.4.8] - 2026-10-29

Fix: Windows checksum command
- Replace Set-Content with `Out-File -NoNewline -Encoding ASCII` to avoid parameter issues

## [0.4.9] - 2026-10-29

Fix: Windows path quoting
- Remove unnecessary escaping so PowerShell sees a valid `D:\...` path

## [0.4.10] - 2026-10-29

Fix: GitHub Release permissions
- Add `permissions: contents: write` so Actions can publish Releases

## [0.5.0] - 2026-10-29

Feature: First-class packaging
- Linux: `.deb` and `.rpm` via nfpm; Arch `mouser-bin` `.pkg.tar.zst` via archlinux container
- Desktop integration: `.desktop`, hicolor icons (PNG + SVG), AppStream metainfo; validated in CI
- macOS: `Mouser.app` bundle and `.dmg`; Homebrew formula (CLI) and Cask (App) generated and pushed
- Windows: Installer `.exe` via Inno Setup; embedded icon/version in exe

### Fixes post-tag
- CI: add `winres` build-dep, remove `docker.io` conflict on Ubuntu runner

## [0.4.1] - 2026-10-29

CI and release automation:
- Add GitHub Actions CI (fmt, clippy, build, tests) on pushes to main across Ubuntu, macOS, Windows
- Add release workflow on `v*` tags: build artifacts for Linux/macOS/Windows and publish GitHub Release with checksums
- Docs: `docs/release.md` and index in `docs/README.md`

## [0.4.0] - 2026-10-29

Feature: Theming
- Add built-in themes (10 popular sets) with dark/light variants
- System/light/dark mode selection; renderer uses theme colors
- Themes written to `~/.mouser/themes` on first run; docs added
- Quality bar clean

## [0.3.0] - 2026-10-29

Feature: Vim-style modes and basic command line
- Add Shell/Command/Config modes (default Shell); command line prompt and actions; Esc returns to Shell
- Toggleable right sidebar via commands; groundwork for menus/keybindings to dispatch actions
- Quality bar clean; docs page added for modes

# Changelog

All notable changes to this project will be documented in this file.

## [0.2.0] - 2026-10-29

Feature: Connection manager foundations and storage path
- Add core storage resolver with `MOUSER_HOME` override and default `~/.mouser`
- Add connection manager data model (SSH, Waypipe, Tunnels, X11) with advanced pass-through options
- Right sidebar listing saved connections (toggle)
- Docs page for connection manager and storage; tests for serialization and arg building

Quality:
- fmt/clippy/test clean; README and AGENTS updated per storage rules

## [0.1.0] - 2026-10-29

Initial release:

- PTY-backed terminal with `vte` parsing
- GPU renderer via `winit` + `wgpu` + `glyphon`
- Chunked, compressed scrollback with Zstd/LZ4 and on-demand decompression
- Scrollback search across compressed history
- Config in TOML for font/colors/scrollback/compression
- Kitty protocol groundwork: keyboard negotiation and basic graphics parsing + placement
- Compliance checklist added under `docs/kitty-compliance.md`

