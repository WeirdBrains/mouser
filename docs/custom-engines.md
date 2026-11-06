# Building a custom rendering engine

Mouser draws through a **shared library with a versioned C ABI**. Your engine
runs in the UI process, presents to the window surface (or an offscreen pixel
buffer) and must keep up with the frame rate. This guide gets a first engine
installed and selected; [Rendering engines](./engines.md) is the full design
and ABI reference.

Official engines ship beside Mouser (`mouser` on wgpu, `skia` on Skia). A
custom engine is the same kind of plugin: a library plus a manifest under
`$MOUSER_HOME/engines/<name>/`.

## What an engine does

Mouser builds a `MouserFrame` each redraw: a clear colour, a dirty rect, and
an ordered list of ops (rectangles, glyph coverage blits, RGBA images). Your
`draw` walks that list and presents. You do **not** parse terminal bytes or
own the scrollback — that stays in Mouser's session process.

Why in-process C ABI (not an out-of-process renderer, not a Rust crate ABI):

- Presenting to the window at 40+ fps without a frame of IPC latency.
- Rust has no stable ABI; C lets engines be written in Rust, C, C++ or Zig.
- A Mouser update does not force an engine rebuild unless the ABI version
  changes.

A library runs with Mouser's privileges. Only install engines you trust. A
crash can still take the window down; Mouser mitigates with a start sentinel
and falls back to the built-in `mouser` engine (see below).

## The entry points

Export one or both symbols. Mouser tries v2 first and falls back to v1:

```c
const MouserEngineV1 *mouser_engine_v1(void);   /* ABI 1 */
const MouserEngineV2 *mouser_engine_v2(void);   /* ABI 2 (Mouser 1.4.0+) */
```

`MouserEngineV1` is `create`, `resize`, `draw`, `destroy`, plus `abi`, `size`
and `name`. `MouserEngineV2` starts with a complete v1 table (`base.abi = 2`)
and adds optional `caps`, `set_scale`, `evict` and `set_vsync`. Frame types
are unchanged. Export both entry points if you want to load in Mouser builds
older than 1.4.0.

Headers and the Rust definitions live in `crates/mouser-engine-abi`
(`mouser_engine.h` and the `mouser-engine-abi` crate). Depend on that crate
from a Rust `cdylib` rather than re-declaring the structs.

Surfaces you may see: Wayland, Xlib, Xcb, AppKit, Win32, and
`SURFACE_PIXELS` (Mouser's engine host / tests fill an offscreen buffer).

## The `flat` example

`crates/mouser-engine-example` is a complete CPU engine named `flat`. It
rasterises rects, glyph coverage and images into a buffer and presents with
softbuffer (or writes `SURFACE_PIXELS`). It also understands test hooks
(`MOUSER_FLAT_FAIL`, `MOUSER_FLAT_CRASH_AT`, `MOUSER_FLAT_ABI`) used by
Mouser's crash-isolation tests.

```sh
cargo build -p mouser-engine-example --release
# Copy the library next to engine.toml, then:
mouser engine install crates/mouser-engine-example
mouser engine use flat
mouser engine list
```

Manifest (`engine.toml` or a `plugin.toml` with `kind = "engine"`):

```toml
name = "flat"
version = "0.1.0"
abi = 1
library = "libmouser_engine_flat.so"   # .dylib / .dll on other OSes
description = "Example engine: CPU raster presented with softbuffer"
```

`install` probes the library in a **separate process** before it replaces
anything on disk. The reported `name` must match the manifest.

## Lifecycle and return codes

1. `create(surface, w, h, err, err_len)` — return an opaque pointer, or NULL
   with a message in `err`. Mouser writes `engines/.starting-<name>` before
   the call and clears it after the first frames present.
2. `resize` — window size changed; next `draw` uses the new size.
3. `draw(frame)` — `0` ok, `1` ask for another redraw, `<0` error (Mouser
   switches to the `mouser` engine and records `.failed-<name>`).
4. `destroy` — free everything; pointers inside a frame are only valid during
   that `draw`.

Colours are `0xRRGGBB`. In ABI v2 the top byte is `255 - opacity` (0 =
opaque), so plain `0xRRGGBB` stays opaque and translucent fills blend.

## Crash protection and fallback

| state | meaning | recovery |
|---|---|---|
| `ok` | nothing recorded | used when selected |
| `skipped` | crashed while starting (sentinel left behind) | `mouser engine unskip <name>` or `use` |
| `failed` | last load/start/draw failed | still tried next start; `use` / reinstall clears |

The built-in `mouser` engine is never skipped — it is the fallback. `skia`
and every custom engine share the sentinel / failed records. Status text in
the window names the reason and the restore command for 15 s.

Live switching (sidebar Engines list, or `mouser engine use`) tears down the
old engine and creates the new one without restarting the session. Details:
[Rendering engines §7](./engines.md).

## Shipping your engine

- Put the library and manifest in a versioned folder
  (`engines/<name>/<version>/` with `current`), or publish a signed archive
  and list it for [Downloads](./downloads.md) (`gh:Owner/Repo`, HTTPS, minisign).
- Prefer ABI 2 and export v1 as well if you care about older Mouser builds.
- Handle `SURFACE_PIXELS` so CI and `mouser engine install`'s probe can run
  without a window.
- Never assume you are the only engine: survive `destroy` + a later `create`
  in the same process after a live switch.

## Checklist

1. `name` in the library matches the manifest.
2. `create` fails cleanly (NULL + message) on an unsupported surface.
3. A deliberate crash during `create` leaves the sentinel and the next start
   uses `mouser` until `unskip`.
4. `mouser engine install` from a clean `MOUSER_HOME` succeeds on your target
   OS.
5. Switching away and back (`use mouser`, then `use <you>`) presents without a
   restart.

## See also

- [Rendering engines](./engines.md) — ABI tables, process split, sidebar
- [Plugins](./plugins.md) — engines are plugins of kind `engine`
- [Downloads](./downloads.md) — signed installs and rollbacks
- [Rendering and fonts](./rendering.md) — what the frame represents
