# Rendering and fonts

Mouser draws text on the CPU (fontdue + softbuffer) on a fixed monospace grid.

Fonts
- Family: `font_family` in config/preferences; otherwise the first installed of JetBrains Mono
  (incl. Nerd Font), Fira Code, Cascadia Mono, DejaVu Sans Mono, Noto Sans Mono, Hack,
  Source Code Pro, Menlo, Consolas, Liberation Mono, Adwaita Mono.
- Fallback: characters missing from the primary font come from symbol/Nerd Font/CJK faces,
  then from the bundled Liberation Mono (SIL OFL 1.1, `crates/mouser-render/assets/fonts`).
- Size: `font_size` (logical px), scaled by the window's scale factor.

Cells
- Cell width is the font's advance; cell height is ascent + descent (+ line gap).
- Every glyph is placed from its own metrics against one shared baseline per row, so letters
  line up and underscores/descenders sit below the text.

Terminal
- The shell is `$SHELL -l`, started at the window's real grid size, with `TERM=mouser`
  when that terminfo entry exists and `xterm-256color` otherwise.
- Supported: deferred autowrap, cursor movement/positioning, erase, insert/delete,
  scroll regions, alternate screen, cursor reports. Colours (SGR) are not drawn yet.
- Scroll back with the mouse wheel; typing returns to the live screen.

## Renderers

Mouser picks a renderer at startup and logs it (`mouser: renderer: ...` on stderr):

- **GPU (default):** a swappable engine, see [engines.md](./engines.md): `mouser`
  (wgpu, below) by default, `skia`, or an installed custom engine; any failure falls
  back to `mouser`. The `mouser` engine is wgpu. Glyphs are rasterised with fontdue into an R8 atlas
  texture; each rect and glyph of the frame's display list is one instanced quad
  with its colour. Blending matches the CPU path.
- **CPU fallback:** softbuffer with the per-size glyph cache. Used when wgpu can't
  get an adapter, device or surface, or when `MOUSER_RENDERER=cpu` is set.

Both are event driven: the loop sleeps in `ControlFlow::Wait` and redraws only on
input or a wake-up from the PTY/config threads, capped at ~120 fps.
