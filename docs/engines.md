# Rendering engines

Mouser draws through a swappable **GPU rendering engine**. The **Mouser engine**
(wgpu) is the default, **Skia** is built in as an option, and **custom engines** can
be installed and removed. Engines only cover GPU rendering: the softbuffer CPU
renderer (`MOUSER_RENDERER=cpu`, or when no GPU is available) is not an engine and
is unchanged.

## 1. What an engine draws

The core already turns the terminal into a resolved frame before anything touches
the GPU: `render_frame` lays out the cell grid, colours, cursor, selection highlight,
underlines, box drawing, the tab bar, panels, the status line and images into a
**display list** (`mouser-render/src/text.rs`, `DisplayList`):

| op | fields |
|---|---|
| clear | colour (0xRRGGBB) |
| rect | x, y, w, h, colour |
| glyph | x, y (top-left of the bitmap), char, colour; the bitmap comes from the core's font stack (8-bit coverage, cached per char and size) |
| image | x, y, drawn width/height, the RGBA8 image (already downscaled by the core when large), a source rect, a cache key; engines scale nearest-neighbour to the drawn size |

So an engine never sees terminal state (grid, scrollback, Fuzzball/Kitty placements,
selection): those are all resolved by the core into the ops above, which keeps every
engine pixel-compatible and makes features (selection, images, new UI) work in all
engines at once. The CPU replay of the same list is the reference image in tests.

Damage: v1 hands over the whole frame each time (the core redraws only when
something changed, at most every 8 ms). A damage rectangle is reserved in the ABI
(`dirty`, currently always the full surface) so engines can be told about partial
updates later without an ABI bump.

## 2. Interface

In Rust (`mouser-render/src/engine.rs`):

```rust
pub trait Engine {
    fn name(&self) -> &str;
    fn description(&self) -> String;                 // e.g. adapter / backend
    fn resize(&mut self, width: u32, height: u32);
    /// Draw and present. Ok(false): surface was reset, draw again.
    fn draw(&mut self, list: &DisplayList, fonts: &mut FontStack) -> anyhow::Result<bool>;
}
```

`FontStack::glyph(ch)` gives the coverage bitmap and metrics for a char at the
current size; `fonts.generation` changes when the font or size changes, so atlases
can be dropped. Built-in engines are created with the window (`Arc<Window>`) and its
size. Custom engines see the same frame through the C ABI below
(`engine::FrameBuf` converts a `DisplayList`).

## 3. Lifecycle

- Engines are kept in a **priority order**: `engines = ["skia", "flat", "mouser"]` in
  `~/.mouser/preferences.toml` (the old single `engine = "x"` key is still read as
  `["x", "mouser"]` and replaced on the next change). `MOUSER_ENGINE=a,b` overrides the
  order for one run. When a window is created Mouser walks the order and uses the
  first engine that starts, passing over skipped ones and ones that fail (each with a
  message); if every GPU engine fails, the CPU renderer is used as before.
- **`mouser` is always in the order** and is the final GPU fallback: it is appended if
  left out, it can be moved up (then engines after it are only reached if it fails),
  it cannot be uninstalled, removed from the order or skipped (no crash marker is
  ever written for it).
- Creation failure (no adapter, missing library, ABI mismatch, `create` error) falls
  back to the Mouser engine, then to the CPU renderer, with a log line and a status
  message saying why.
- A draw error at run time switches the window to the next engine after it in the order
  (mouser at the latest), with a status-bar message.
- **No hot swap in v1**: `mouser engine use NAME` changes the preference; new windows use
  it, open windows keep their engine until restarted. Swapping a live surface between
  APIs (wgpu, GL) is possible but needs careful surface teardown on Wayland; it can be
  added later without changing engines.

## 4. Custom engines: C ABI dynamic libraries

Choice: a **shared library with a stable, versioned C ABI** (`cdylib`), loaded with
`libloading`. Reasons:

- Rendering must present to the window's own surface at 40+ fps; an out-of-process
  engine would need surface sharing (dmabuf/Wayland subsurfaces, per platform) and a
  frame of latency for every op list. In-process is simple and fast.
- Rust has no stable ABI, so the boundary is plain C: engines can be written in Rust,
  C, C++ or Zig, and a Mouser update does not require rebuilding engines unless the
  ABI version changes.
- Safety: a library runs with Mouser's privileges, so only install engines you trust
  (same as a shell plugin). Mouser guards what it can (below), but a crashing engine
  can still take the window down.

The library exports one symbol:

```c
const MouserEngineV1 *mouser_engine_v1(void);

typedef struct {
    uint32_t abi;            /* MOUSER_ENGINE_ABI = 1 */
    uint32_t size;           /* sizeof(MouserEngineV1), for additive growth */
    const char *name;        /* engine name, must match the manifest */
    void *(*create)(const MouserSurface *surface, uint32_t w, uint32_t h,
                    char *err, size_t err_len);      /* NULL on failure */
    void  (*resize)(void *engine, uint32_t w, uint32_t h);
    int   (*draw)(void *engine, const MouserFrame *frame); /* 0 ok, 1 redraw, <0 error */
    void  (*destroy)(void *engine);
} MouserEngineV1;

typedef struct { uint32_t kind;  /* 1 Wayland, 2 Xlib, 3 Xcb, 4 AppKit, 5 Win32 */
                 void *display; void *window; } MouserSurface;

typedef struct { int32_t x, y, w, h; uint32_t color; } MouserRect;
typedef struct { int32_t x, y; uint32_t w, h, stride, color; const uint8_t *coverage;
                 uint64_t key; } MouserGlyph;        /* key: stable per (char, font generation) */
typedef struct { int32_t x, y; uint32_t w, h;          /* drawn size */
                 uint32_t img_w, img_h, src_x, src_y, src_w, src_h;
                 const uint8_t *rgba; uint64_t key; } MouserImage; /* RGBA8, straight alpha */
typedef struct { uint32_t kind; uint32_t index; } MouserOp; /* 0 rect, 1 glyph, 2 image */

typedef struct {
    uint32_t width, height, clear;
    int32_t dirty_x, dirty_y, dirty_w, dirty_h;   /* v1: always the whole surface */
    const MouserOp *ops; size_t op_count;         /* painting order */
    const MouserRect *rects; size_t rect_count;
    const MouserGlyph *glyphs; size_t glyph_count;
    const MouserImage *images; size_t image_count;
} MouserFrame;
```

All pointers in a frame are valid only during `draw`. Colours are 0xRRGGBB. The
header is `crates/mouser-engine-abi/mouser_engine.h`; the Rust side of the ABI is the
`mouser-engine-abi` crate, which engine authors can depend on.

Guards on the Mouser side: the ABI number must match and `size` must be at least the
v1 table, the reported name must match the manifest (checked at install and at load),
a negative `draw` result switches the window to the Mouser engine, and a **crash sentinel**: before calling `create` Mouser writes
`engines/.starting-<name>`, and removes it after the first frames are presented. If
Mouser finds the sentinel at the next start (the engine crashed the process), it skips
that engine, uses the Mouser engine and says so (`mouser engine list` shows it too);
`mouser engine use <name>` or reinstalling clears it.

### Skipped and failed engines

`mouser engine list` shows a state per engine:

| state | meaning | recorded in | what Mouser does |
|---|---|---|---|
| `ok` | nothing recorded | | uses it when selected |
| `skipped` | crashed Mouser while starting (sentinel left behind), with when | `engines/.starting-<name>` | skips it at every start until restored |
| `failed` | the last load/start/draw attempt failed, with when and why | `engines/.failed-<name>` | still tries it on each start (shown so you know why you got `mouser`) |

`mouser engine unskip <name>` (alias `restore`; `--all` for every engine) clears both
records **without selecting** the engine; `mouser engine use <name>` clears them and
selects it; reinstalling clears them too. A successful start clears `failed`. Built-in
`skia` is guarded the same way as custom engines (`mouser` never is: it is the fallback).

When Mouser falls back it prints and logs the reason and the restore command, and shows
it in the window's status bar for 15 s, e.g.
`engine flat is skipped: it crashed Mouser while starting (2m ago). Restore it with
`mouser engine unskip flat`; using mouser`.

Why not XDG (`$XDG_DATA_HOME/mouser/engines`)? Mouser keeps all of its state under
one directory, `~/.mouser` or `$MOUSER_HOME` (docs/storage.md), so engines follow
that rule; tests point `MOUSER_HOME` (and `HOME`) at a temporary directory.

## 5. Installing

Installed engines live in **`$MOUSER_HOME/engines/<name>/`** (default
`~/.mouser/engines`), following Mouser's fixed storage rule (all state under
`~/.mouser`, `MOUSER_HOME` override; tests always use a temporary `MOUSER_HOME`). Each
holds an `engine.toml` manifest and the library:

```toml
name = "flat"
version = "0.1.0"
abi = 1
library = "libmouser_engine_flat.so"
description = "Example engine: solid rectangles and glyph boxes"
```

CLI (thin adapters over `mouser_core::engines`):

| command | does |
|---|---|
| `mouser engine list` | built-in (`mouser`, `skia` if compiled in) and installed engines, the active one marked, broken ones with the reason |
| `mouser engine install <dir \| engine.toml \| lib.so>` | checks the manifest, loads the library and checks its ABI, copies it into `engines/<name>/` |
| `mouser engine uninstall <name>` | removes `engines/<name>/` (built-ins cannot be removed); if it was in use, the config goes back to `mouser` |
| `mouser engine use <name>` | moves it first in the order and unskips it |
| `mouser engine priority <a> <b> ...` | sets the whole order (`mouser` appended if missing) |
| `mouser engine move <name> first\|last\|up\|down\|<n>` | moves one engine (1 = first) |
| `mouser engine unskip <name>\|--all` (alias `restore`) | clears skip/failure records, order unchanged |

Installing from a URL is not supported in v1: download the engine yourself and install
the directory (no network access from the terminal, no unsigned code fetched for you).

## 6. Built-in engines

- `mouser`: the wgpu renderer (Vulkan/GL/Metal/DX12): instanced quads, a glyph atlas,
  image textures cached by key. Unchanged; it is now behind the trait.
- `skia`: Skia (Ganesh) over OpenGL through glutin (EGL, then GLX; CGL on macOS, WGL
  on Windows). Rects are Skia rects; glyphs go into one A8 atlas image drawn with the
  glyph colour (Skia tints alpha-only images and batches quads sharing a texture);
  images are Skia images cached by key, drawn nearest-neighbour. Swaps without
  waiting for vblank (`SwapInterval::DontWait`): on Wayland/NVIDIA a vsynced EGL swap
  blocked ~30 ms per frame during a drag-resize; Mouser already caps frames at one per
  8 ms. Built behind the `skia` cargo feature of `mouser-app`, **on by default**
  (the `skia-safe` crate downloads a prebuilt, statically linked Skia at build time;
  no system packages; the binary then needs `libstdc++` and, to use Skia, `libEGL`
  or `libGL`). `cargo build --no-default-features` leaves it out.
- Example custom engine: `crates/mouser-engine-example` (`flat`), a `cdylib` that
  rasterises the ABI frame on the CPU and presents it with softbuffer. Tests build it
  and use it for install/list/use/uninstall, ABI mismatch and load-failure fallback;
  `tests/engine_frame_tests.rs` checks its output against the CPU reference image.

```bash
cargo build --release -p mouser-engine-example
mkdir flat && cp target/release/libmouser_engine_flat.so crates/mouser-engine-example/engine.toml flat/
mouser engine install flat && mouser engine use flat
```

## 7. Status and gaps

Measured on alien-blue (RTX 3060, Hyprland, 1100x760, the multi-image screen,
~240 drag-resize steps): mouser 57 fps (frame p95 8.9 ms), skia 59–60 fps (p95
12.8–13.1 ms), flat 59 fps (p95 11–12 ms). The sweep issues one resize per ~16 ms, so
~60 fps is the ceiling of the test. Skia's output matches the CPU reference to a few
hundred pixels (cursor/rounding); the wgpu engine differs slightly more (its blending
was already a little different).

Gaps: no hot swap; no URL install; no per-engine options; damage is always the full
frame; the frame has no text runs or font data (engines can't do their own shaping or
subpixel text, by design for v1); a crashing custom engine still takes the window down
(the sentinel only protects the next start).
