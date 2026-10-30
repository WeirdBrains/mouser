# Mouser — a fast GPU terminal emulator (Linux-first)

<p align="center"><img src="resources/icons/png/mouser_256.png" alt="Mouser icon" width="128" height="128"/></p>


Mouser is a new terminal emulator written in Rust. It runs your login shell inside a PTY, parses VT/xterm escape sequences, and renders a text grid with the GPU. Linux is the primary target for now; other platforms may work via `portable-pty` and `winit` but are not tested yet.

## Highlights

- Real PTY-backed terminal with `vte` parsing
- Fast text rendering via `winit` + `softbuffer` + `fontdue` (CPU rasterization, zero-copy blits)
- Chunked, compressed scrollback using Zstd or LZ4, with idle-based auto-compression and transparent on-demand decompression
- Scrollback search across compressed history
- Kitty protocol support (keyboard + graphics) with a public compliance checklist
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

## Kitty protocol support

Mouser aims for full support over time. Today’s status lives in [`docs/kitty-compliance.md`](docs/kitty-compliance.md) and is summarized here:

- Keyboard protocol: negotiation commands recognized; extended `CSI u` key reporting enabled when requested.
- Graphics protocol: parses Kitty graphics sequences and iTerm2-compatible `OSC 1337` image transfers; basic placement and deletion supported. More features tracked in the checklist.
- Extensions: OSC 8 hyperlinks, OSC 52 clipboard, synchronized output (2026), OSC 99 desktop notifications — initial parsing and handling where feasible; details in the checklist.

If a feature your workflow depends on is missing, please open an issue referencing the checklist item.

## Configuration

Mouser keeps everything in its home directory: `~/.mouser`, or `$MOUSER_HOME` if set
(`--config-dir DIR` overrides it for `config.toml`; `--config FILE` points at one file).

- `config.toml` — static settings: fonts, scrollback, compression.
- `preferences.toml` — choices made in the app: `theme`, `mode` (`dark`/`light`/`system`),
  `font_family`/`font_size` overrides, and the sidebar's `sidebar_open`, `sidebar_collapsed`,
  `sidebar_width`. Values here win over `config.toml`.
- `themes/<Theme>/{dark,light}.toml` — theme colours.

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

See `docs/release.md` for details and per-platform install commands.
Tests (with a safety timeout):

```bash
timeout 120 cargo test --workspace
```

## Kitty compliance checklist

See [`docs/kitty-compliance.md`](docs/kitty-compliance.md). We keep this updated as we implement features. Tests for keyboard and graphics parsers live under `mouser-core/tests/`.

## Roadmap

- Rich text shaping and ligatures
- Tabs and multiple terminals
- Full Kitty graphics feature coverage, animations, and shared memory transport
- Cross-platform validation (Windows/macOS)

## License

MIT

