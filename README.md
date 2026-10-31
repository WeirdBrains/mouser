# Mouser — a fast GPU terminal emulator (Linux-first)

<p align="center"><img src="resources/icons/png/mouser_256.png" alt="Mouser icon" width="128" height="128"/></p>


Mouser is a new terminal emulator written in Rust. It runs your login shell inside a PTY, parses VT/xterm escape sequences, and renders a text grid with the GPU. Linux is the primary target for now; other platforms may work via `portable-pty` and `winit` but are not tested yet.

## Highlights

- Real PTY-backed terminal with `vte` parsing
- Fast text rendering via `winit` + `softbuffer` + `fontdue` (CPU rasterization, zero-copy blits)
- Chunked, compressed scrollback using Zstd or LZ4, with idle-based auto-compression and transparent on-demand decompression
- Scrollback search across compressed history
- Fuzzball protocol (graphics + keyboard, `docs/protocol/`), with the Kitty keyboard and Kitty graphics protocols as fallbacks
- TOML configuration for font, colors, scrollback size, compression algorithm/level, and idle thresholds

## Why these crates?

- `portable-pty`: robust, cross-platform PTY spawning also used by other modern terminals. On Linux it integrates cleanly with native PTYs and shells.
- `vte`: a small, fast xterm/VT parser with a simple `Perform` interface we can implement to update our screen and scrollback.
- `winit` + `softbuffer`: portable windowing with a simple, efficient software framebuffer for Linux; this keeps first runs lightweight and avoids driver wrinkles.
- `fontdue` + system fonts via `fontdb`: fast CPU glyph rasterization with a simple API. We can swap in GPU text later if needed.
- `zstd` and `lz4_flex`: high-performance compression backends. Zstd is the default; LZ4 is available for lower-latency trade-offs.

## Architecture

Workspace crates:

- `mouser-core`: terminal core — grid/screen model, `vte` parser handler, scrollback storage with chunked compression and search.
- `mouser-pty`: PTY spawn and I/O glue (shell discovery, pty sizing, child lifecycle).
- `mouser-render`: window + GPU text renderer (`winit`, `wgpu`, `glyphon`) and input handling, viewport scrolling and prefetch.
- `mouser-app`: the binary that ties everything together: config loading, runtime wiring, PTY reader thread, and UI loop.

Data flow:

1. `mouser-pty` spawns the user's shell in a PTY and yields reader/writer handles.
2. A reader thread forwards PTY bytes to the main thread via channels.
3. `mouser-core`'s `vte` handler parses bytes, mutating the screen grid and appending lines into the scrollback store.
4. The renderer asks `mouser-core` for the visible region (respecting viewport offset), which transparently decompresses any needed chunks and prefetches neighboring blocks.
5. Idle maintenance (on each UI tick) compresses old, not-recently-viewed chunks; the live screen and recent tail stay uncompressed.

## Scrollback compression design

- Lines accumulate in fixed-size chunks (default 1024 lines per chunk).
- When a chunk has not been accessed for `idle_compress_ms` (default 15000 ms), it is compressed in-place (default Zstd level 3).
- Decompression is on-demand when the viewport overlaps a compressed chunk or when search traverses it.
- To avoid UI stalls, the renderer prefetches ±1 chunk around the viewport and schedules recompression after those chunks again go idle.
- The maximum scrollback is bounded (`max_scrollback_lines`) and drops the oldest chunks when exceeded.

## Fuzzball and Kitty fallbacks

- **Fuzzball** is Mouser's own protocol for images and structured key events (DCS, tag `Fz`,
  tmux/screen-friendly). Spec: [`docs/protocol/`](docs/protocol/README.md). Try it with
  `tools/fuzzball-demo.py` inside Mouser.
- **Kitty keyboard protocol** is supported as a fallback, for apps that don't speak
  Fuzzball yet. Compliance matrix: [`docs/kitty-compliance.md`](docs/kitty-compliance.md).
- **Kitty graphics protocol** is supported as a fallback too (`kitten icat`, chafa, yazi, …),
  including Unicode placeholders and animation; Fuzzball takes precedence. Details and
  limits: [`docs/protocol/graphics.md` §7](docs/protocol/graphics.md).
- iTerm2 `OSC 1337` images are **not** supported.

### Showing images: `mouser show`

```sh
mouser show photo.jpg diagram.png      # one after another, each scaled to the terminal width (follows resizes)
curl -s https://example.com/x.png | mouser show -
mouser show --width 30 logo.png        # 30 cells wide, height keeps the aspect
mouser show --original icon.png        # natural pixel size
```

Formats: PNG (sent as-is), JPEG, GIF (first frame), WebP, BMP. Video and audio are not
supported. Uses Fuzzball graphics when the terminal answers, Kitty graphics otherwise.
Inside tmux (`set -g allow-passthrough on`) it shows Kitty Unicode placeholders, which
tmux moves and redraws like text. Exits non-zero when neither protocol answers and for
unsupported files.

## Tabs

Each tab has its own PTY, screen, scrollback and title (OSC 0/2, otherwise the
connection name). Clicking a connection in the sidebar opens a new tab running
`ssh [-F config] <alias>` directly; Waypipe, tunnel and X11 launches open in
tabs too. The tab bar is hidden while only one tab is open.

| Keys | Action |
|---|---|
| Ctrl+Shift+T | New shell tab |
| Ctrl+Shift+W | Close the active tab |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous tab |
| Alt+1 … Alt+9 | Jump to tab N |
| Click / middle-click / × | Switch / close a tab |

A tab closes when its process exits, and the window closes with the last tab.
An ssh tab that exits with an error stays open showing
`[process exited N] press any key to close`.

## Terminal

- **Drag** selects text (double-click word, triple-click line, Alt+drag block, Shift+click extends); **Ctrl+Shift+C** copies.
- **Ctrl+Shift+V** pastes the clipboard (bracketed when the app asks for it); **middle-click** / **Shift+Insert** paste the selection (PRIMARY). On macOS use Cmd+C / Cmd+V.
- **Ctrl+click** opens an OSC 8 hyperlink.
- **Shift+mouse** bypasses an app's mouse capture.
- Resizing re-wraps long lines (screen and scrollback); images move with their text.
- Install the Mouser terminfo with `tic -x -o ~/.terminfo extra/mouser.terminfo`; Mouser then sets `TERM=mouser` (otherwise `xterm-256color`).

Run a program instead of the shell with `mouser -e btop` or `mouser -- nvim file`;
`--working-directory DIR` (`--dir`), `--title`, `--class` (`--app-id`), `--hold` and
`--version` work like in other terminals. The desktop entry is
xdg-terminal-exec compliant, so Mouser can be the default terminal on Omarchy and other
desktops (`mouser.desktop` first in `~/.config/xdg-terminals.list`).

See [docs/terminal.md](docs/terminal.md) for the supported sequences and options.

## Configuration

Mouser keeps everything in its home directory: `~/.mouser`, or `$MOUSER_HOME` if set
(`--config-dir DIR` overrides it for `config.toml`; `--config FILE` points at one file).

- `config.toml` — static settings: fonts, scrollback, compression.
- The sidebar comes back the way you left it: its open/collapsed state and width are saved
  to `preferences.toml` whenever they change and when the window closes (including when the
  shell exits). A fresh install starts with the collapsed strip (click `«` to expand).
  `MOUSER_OPEN_SIDEBAR=1` forces it open and expanded for that run without overwriting the
  saved state.
- Rendering uses the GPU (wgpu) when available and falls back to the CPU; set
  `MOUSER_RENDERER=cpu` to force the CPU renderer. See `docs/rendering.md`.
- `preferences.toml` — choices made in the app: `theme`, `mode` (`dark`/`light`/`system`),
  `font_family`/`font_size` overrides, and the sidebar's `sidebar_width`, `sidebar_open` and
  `sidebar_collapsed`. Values here win over `config.toml`.
- `themes/<Theme>/{dark,light}.toml` — theme colours (foreground, background, cursor, accent, palette).

SSH connections live in `~/.ssh/config` (override with `MOUSER_SSH_CONFIG`); Mouser edits it in
place and keeps a backup at `~/.ssh/config.mouser.bak`. See `docs/connection-manager.md`.

Change the theme and font in the app: sidebar → **Appearance** (or `:theme`, `:mode`, `:edit-theme`
from the F10 command line), and Ctrl+= / Ctrl+- / Ctrl+0 for font size. See `docs/themes.md`.

`config.toml` example:

```toml
[ui]
font_family = "JetBrains Mono"
font_size = 14.0

[scrollback]
max_scrollback_lines = 200000
chunk_lines = 1024
idle_compress_ms = 15000
prefetch_chunks = 1
search_case_sensitive = false

[compression]
algorithm = "zstd"   # zstd | lz4 | none
level = 3
```

`preferences.toml` example:

```toml
theme = "Red Star"
mode = "system"
sidebar_open = true
sidebar_width = 320
```

## Build and run

Prerequisites on Linux:

- X11 or Wayland libraries as required by `winit`.

Build:

```bash
cargo build --workspace
```

Run:

```bash
cargo run -p mouser-app --release
```

Prebuilt install packages are attached to each GitHub Release:
- Linux: `.deb` (Debian/Ubuntu), `.rpm` (Fedora), and an Arch `mouser-bin` `.pkg.tar.zst`
- macOS: CLI Homebrew formula (`Formula/mouser.rb`) and a `Mouser.app` `.dmg` with a Cask (`Casks/mouser.rb`)
- Windows: portable `.zip` and an installer `.exe`

### Arch Linux

```bash
# Latest release (prebuilt):
sudo pacman -U https://github.com/WeirdBrains/mouser/releases/latest/download/mouser-bin-<ver>-1-x86_64.pkg.tar.zst
# Or build main from source:
git clone https://github.com/WeirdBrains/mouser && cd mouser/packaging/arch/mouser-git && makepkg -si
```

`yay -S mouser-git` / `yay -S mouser-bin` once the packages are on the AUR (planned).
Both replace each other and an older `mouser-bin` install (pacman asks to remove it).

See `docs/release.md` for details and per-platform install commands.
Tests (with a safety timeout):

```bash
timeout 120 cargo test --workspace
```

## Kitty keyboard compliance

See [`docs/kitty-compliance.md`](docs/kitty-compliance.md). Tests live in `crates/mouser-core/tests/kitty_keyboard_*`.

## Roadmap

- Rich text shaping and ligatures
- Tabs and multiple terminals
- Fuzzball: IME events
- Cross-platform validation (Windows/macOS)

## License

MIT

