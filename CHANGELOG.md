## [Unreleased]

Added:
- Fuzzball protocol v0.1 (`docs/protocol/`): image upload/place/delete/query, keyboard mode with structured key events, capability query (`DCS = F z c q`, XTGETTCAP `Fz`)
- Golden-pixel tests for Fuzzball images (CPU and the GPU display list)
- `tools/fuzzball-demo.py` demo client
- Fuzzball placements follow their text through scroll regions, SU/SD, IL/DL, the scrollback and resize; they are removed when their anchor row leaves the scrollback, scrolls off the alternate screen
- Fuzzball images scale to fit the screen width on resize (and when placed), move left when their anchor column is cut off, and grow back on a wider screen; downscaled images are area-averaged and cached for both renderers; cell-size changes rescale their pixels
- Abandoned Fuzzball uploads are discarded by a timer after 10 s
- ED 3 (`CSI 3 J`) clears the scrollback (and the placements in it) without touching the screen

Changed:
- Kitty keyboard protocol is the only Kitty protocol, kept as a fallback behind Fuzzball keyboard mode
- Kitty keyboard: Ctrl/Alt/Super keys carry no associated text (flag 16)
- Kitty keyboard compliance fixes: F13+/media/keypad keys, lock bits on text keys, Enter/Tab/Backspace releases, Ctrl+Backspace, Ctrl+Shift/Super text keys in legacy mode; tests per spec section

Removed:
- Kitty graphics protocol (APC `_G`)
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

