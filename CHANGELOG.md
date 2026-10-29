## [0.4.2] - 2026-10-29

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

