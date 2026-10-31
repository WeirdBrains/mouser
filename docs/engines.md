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
| image | x, y, drawn width/height, RGBA pixels already scaled to the drawn size (area-averaged), a cache key, a source rect |

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
    fn draw(&mut self, list: &DisplayList, glyphs: &mut dyn Glyphs) -> anyhow::Result<bool>;
}
```

`Glyphs` gives an engine the coverage bitmap and metrics for a char at the current
font size, plus a generation number that changes when the font or size changes (so
atlases can be dropped). Engines are created with the window (`Arc<Window>`) and its
size; built-in engines use winit/raw-window-handle directly.

## 3. Lifecycle

- The engine is chosen when a window is created: `MOUSER_ENGINE=<name>` (testing),
  else `[render] engine = "<name>"` in `config.toml`, else `mouser`.
- Creation failure (no adapter, missing library, ABI mismatch, `create` error) falls
  back to the Mouser engine, then to the CPU renderer, with a log line and a status
  message saying why.
- A draw error at run time also drops to the Mouser engine for that window (logged).
- **No hot swap in v1**: `mouser engine use NAME` changes the config; new windows use
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
typedef struct { int32_t x, y; uint32_t w, h; const uint8_t *rgba; uint64_t key; } MouserImage;
typedef struct { uint32_t kind; uint32_t index; } MouserOp; /* 0 rect, 1 glyph, 2 image */

typedef struct {
    uint32_t width, height, clear;
    int32_t dirty_x, dirty_y, dirty_w, dirty_h;   /* v1: always the whole surface */
    const MouserOp *ops; size_t op_count;         /* painting order */
    const MouserRect *rects; const MouserGlyph *glyphs; const MouserImage *images;
} MouserFrame;
```

All pointers in a frame are valid only during `draw`. Colours are 0xRRGGBB. The
header is `crates/mouser-engine-abi/mouser_engine.h`; the Rust side of the ABI is the
`mouser-engine-abi` crate, which engine authors can depend on.

Guards on the Mouser side: the ABI number and `size` must match, the name must match
the manifest, every call is wrapped so a Rust panic in Mouser's shim cannot unwind
into the library, and a **crash sentinel**: before calling `create` Mouser writes
`engines/.starting-<name>`, and removes it after the first frames are presented. If
Mouser finds the sentinel at the next start (the engine crashed the process), it skips
that engine, uses the Mouser engine and says so; `mouser engine use <name>` clears it.

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
| `mouser engine use <name>` | sets `[render] engine` in `config.toml` (checks it exists) |

Installing from a URL is not supported in v1: download the engine yourself and install
the directory (no network access from the terminal, no unsigned code fetched for you).

## 6. Built-in engines

- `mouser`: the wgpu renderer (Vulkan/GL/Metal/DX12): instanced quads, a glyph atlas,
  image textures cached by key.
- `skia`: Skia through OpenGL (glutin on the window). Rects as Skia rects, glyphs as
  alpha-mask images tinted by colour, images as Skia images cached by key. Built
  behind the `skia` cargo feature (the `skia-safe` crate downloads prebuilt Skia).
- Example custom engine: `crates/mouser-engine-example` (`flat`), built as a cdylib, used
  by the tests for install/load/fallback.
