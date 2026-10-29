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

