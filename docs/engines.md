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
- **Live switching**: running windows switch engine in place when the order (or a
  skip mark) changes; see §7.

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

## 7. Switching live and surviving engine crashes (design, t141u)

### Requirements

Switching engines must keep everything the user has (tabs, running programs and
their PTYs, scrollback, cursor, selection, images). An engine that crashes must not
take the user's shells with it: the window carries on with the next engine in the
order and the crashed one is marked skipped (restorable with `unskip`).

### Options considered

1. **In-process engines + signal handler.** Catching SIGSEGV and continuing is UB;
   rejected.
2. **Holder process.** A small process owns the PTYs; the UI (parser, scrollback,
   images, window, engine) is a separate process that reattaches after a crash. It
   protects against every crash, including the wgpu/Skia drivers, but the UI holds all
   terminal state, so after a crash it would have to rebuild that state by replaying a
   byte log of every PTY (bounded, slow for big scrollbacks, images re-decoded), or the
   holder would have to own the parser and stream screens to the UI (a tmux-style
   client/server split of the whole terminal). Too big and too risky for this phase;
   kept as the long-term option.
3. **Out-of-process custom engines (chosen).** A custom engine runs in an **engine
   host** child process (`mouser __engine-host`). Mouser sends it each frame (ops,
   rects, glyph bitmaps and images, each bitmap/image sent once and cached by key) over
   a pipe; the engine draws into a pixel buffer (`MOUSER_SURFACE_PIXELS`, a new
   surface kind: `window` points to a `MouserPixels { data, width, height, stride }`);
   the host returns the pixels and Mouser presents them. If the host dies (segfault,
   abort, panic, `exit`), Mouser sees EOF on the pipe: the engine is marked skipped
   (with the reason), and the window switches to the next engine in the order on the
   same frame. All state lives in the main process, so nothing is lost.

### Built-in engines

`mouser` (wgpu) and `skia` stay in-process for direct GPU presentation (the reason
they are built in). They are covered as far as is sound:

- every `create`/`draw` call runs under `catch_unwind`, so a Rust panic (ours, wgpu's,
  skia-safe's) becomes an error: the engine is marked skipped and the window moves on;
- wgpu device/surface loss and Skia/GL errors are errors, not crashes: same path;
- a native crash inside the GPU driver or Skia's C++ still ends the process. The
  crash marker (§4) then makes the next start skip that engine. This is the
  remaining limit; the holder-process design above is what would remove it.

### Switching

Engines are swapped **on the same window**: the old engine is dropped (its surface is
released), the new one is created on the window, the window is redrawn. Sessions,
PTYs and all terminal state are untouched because they never belonged to the engine;
the window keeps its size and position (nothing for the compositor to redo). If the
new engine cannot start on the existing window, the next one in the order is tried
(mouser last, then the CPU renderer).

Running windows learn about a change by watching `preferences.toml` (checked twice a
second): when the effective first engine of the priority order changes (`mouser engine
use`, `priority`, `move`, `unskip`), each window switches. No socket is needed; an
`MOUSER_ENGINE` override pins a window's order for its lifetime.

### Pixel-buffer engines: costs

The pipe carries one frame of ops (tens of KB) and returns width x height x 4 bytes
(3.3 MB at 1100x760); measured below. GPU custom engines must read back their image
(the price of isolation). An engine that cannot draw into pixels fails `create` with
that surface and is passed over. Windows has no engine host yet (custom engines stay
in-process there).

### As built

- Host: `mouser-render/src/engine_host.rs` (`serve`, `HostClient`, `HostedEngine`,
  `EngineCrashed`), started as `mouser __engine-host <library> <name>`. A watchdog kills
  a host whose frame takes over 5 s (a hang counts as a crash). On Windows/without the
  `mouser` binary custom engines stay in-process with the crash marker.
- A crash or panic writes the skip mark with the reason (`engines::mark_skipped`), e.g.
  `engine flat crashed in its engine host (signal 6)`, shown by `mouser engine list`;
  the window walks on to the next engine of the order at once.
- The start marker of an engine that is still starting in this process is not a crash
  (it holds the pid); a window that switches away before 3 frames removes it.
- Live test (alien-blue): a shell printing its pid and a tick each second kept the same
  pid and kept ticking through mouser -> skia -> flat (`mouser engine use`/`priority`
  from another terminal), an injected crash of flat (`MOUSER_FLAT_CRASH_AT`) -> skia
  with the status `engine flat crashed in its engine host (signal 6); your shells are
  untouched; skipped until mouser engine unskip flat`, and `mouser engine unskip flat`
  -> back to flat. Mouser itself never restarted.

Limits: a native crash in the GPU driver, wgpu or Skia's C++ (built-ins are in-process)
still ends Mouser (next start skips that engine); an engine host shows its output via
softbuffer (one copy of each frame); the selection is kept by switching (it is core
state) but nothing is preserved across a whole-Mouser crash.

## 8. Session/UI process split (design, t145u)

Goal: a native crash in **any** engine (GPU driver, wgpu, Skia's C++, a custom engine)
never loses shells, scrollback, tabs or images. Rollback point: tag `pre-session-split`
(adf349b).

### Where the line is

Mouser already has a narrow, data-only boundary between the terminal and the window:
`RenderHooks` (mouser-render/src/lib.rs). The renderer loop asks for the view
(`fetch_view`, `get_images`, `get_cursor`, `get_colors`, `get_font`, `get_tabs`,
`get_ui_model`, `get_status`, `get_title`, ...) and reports input (`on_key_event`,
`on_mouse`, `on_select`, `on_resize`, `on_ui_action`, `on_paste`, ...). Everything
behind the hooks (sessions, PTYs, parser, scrollback, graphics, selection, sidebar
model, preferences) is the **server**; the winit loop + engine is the **UI**. The split
turns the hooks into calls over a socket. No terminal code moves, so behaviour stays the
same and the single-process mode (Windows, `MOUSER_SPLIT=0`) keeps working unchanged.

### Processes and lifecycle

- `mouser` (the process the user starts) is the **server**: it parses the command line,
  starts the sessions as today, then spawns the **UI** child `mouser __ui` and serves it.
- One server per Mouser window (as today: one process per window). No daemon, so
  nothing to discover: the UI gets two connected `socketpair` sockets as its stdin and
  stdout (calls, events). No socket path exists, so there are no permissions to get
  wrong and no stale sockets; another user cannot connect. (A per-user server with a
  socket in `$XDG_RUNTIME_DIR/mouser/` (0700) for re-attaching from a new launch is
  possible later on top of the same protocol.)
- The UI is started from `/proc/self/exe` on Linux (the exact binary the server runs,
  even after a package upgrade replaced the file), `current_exe()` elsewhere. Both
  sides send a protocol version in the hello; on mismatch the server falls back to
  running the UI in-process (single-process mode) and says so.
- **UI exit codes**: 0 after a normal close (the user closed the window: the server
  ends, like today); anything else, or a signal, is a **UI crash**.
- **On a UI crash** the server marks the engine the UI last reported as skipped
  (`engines::mark_skipped`, reason e.g. `UI process crashed (signal 11) while drawing with
  skia`) and starts a new UI immediately, which walks the priority order as usual and
  so uses the next engine. If the crashed engine was `mouser` (never skipped), the new
  UI uses the CPU renderer (`MOUSER_RENDERER=cpu`). More than 5 crashes in 30 s: the
  next UI also uses the CPU renderer; if that crashes too the server gives up and exits
  (sessions end, as a crash does today).
- The server exits when its last session ends (it tells the UI to close) or when the
  window is closed.

### Protocol

Length-prefixed bincode messages.

- **Calls** (UI -> server, request/response, one at a time): one message per hook with
  the hook's arguments; the reply is its return value. Images: the reply carries each
  placement's pixels only the first time the UI sees its key (the UI caches by key and
  drops unused keys like the GPU engines do).
- **Events** (server -> UI, pushed): `Wake` (redraw; coalesced), `Exit`, and clipboard
  requests `ClipStore { text, primary }` / `ClipLoad { primary }` answered by the UI.
  The clipboard lives in the UI because Wayland clipboard access needs the window's
  connection and keyboard focus.
- UI -> server notifications: `Engine(name)` whenever the UI's engine changes (so the
  server knows what to skip after a crash), `Geometry(w, h)` (size for the next UI).

State sync: there is no screen-state replication to keep in sync. The UI holds no
terminal state, only per-window input state (scroll position, drag, hover) and caches
(glyph atlas, image textures). A new UI starts empty and asks for the current view on
its first frame, so "full snapshot on attach" is simply the first frame; afterwards every
frame asks again (the view for the visible rows only, ~tens of KB). Damage tracking is
still the full frame (as before the split).

Resize: the UI sends `on_resize` (cols, rows, pixels) as before; the server resizes the
PTYs. Input: key/mouse/selection events are hook calls; their replies are immediate.

### Latency budget

A frame makes ~12 calls; a socketpair round trip is ~10–30 µs, so ~0.3 ms per frame,
well inside the 25 ms (40 fps) budget. Keystroke-to-frame adds one call for the key and
the next frame's calls. Measured before/after (§9).

### Engine host

Kept for custom engines: inside the UI it turns a custom-engine crash into an in-place
switch (no window re-creation, no flash), which is nicer than a UI restart. The UI
restart covers everything the host doesn't (built-in engines, the host itself).

### Platforms

Linux and macOS: split by default. Windows: no socketpair/fd inheritance in std; stays
single-process (custom engines still in-process) for now.

### What a UI crash still loses

The window itself (it is re-created: the compositor decides the position; the size is
kept), scroll-back position of the view (back at the bottom), an in-progress mouse drag,
the glyph/image caches (rebuilt). Shells, programs, scrollback, tabs, images, selection,
sidebar state and preferences are in the server and survive.

## 9. Status and gaps


Measured on alien-blue (RTX 3060, Hyprland, 1100x760, the multi-image screen,
~240 drag-resize steps), after isolation: mouser 62–63 fps (frame p95 7.0 ms), skia
63 fps (p95 12.4–13.4 ms), flat in the engine host 61.7 fps (p95 15 ms). The sweep issues one resize per ~16 ms, so
~60 fps is the ceiling of the test. Skia's output matches the CPU reference to a few
hundred pixels (cursor/rounding); the wgpu engine differs slightly more (its blending
was already a little different).

Gaps: no URL install (by decision: download, then install the folder); no per-engine
options; damage is always the full frame; the frame has no text runs or font data
(engines can't do their own shaping or subpixel text, by design for v1); native crashes
in built-in engines (see §7).
