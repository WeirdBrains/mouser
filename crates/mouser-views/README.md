# mouser-views

Views for [Mouser](https://github.com/WeirdBrains/mouser) mode plugins, drawn
as [`mouser-mode-api`](https://crates.io/crates/mouser-mode-api) panel items:

- `Layout`: list, grid or carousel;
- `grid::Grid`: tiles in rows that scroll to keep the selection on screen;
- `carousel::Strip`: the selection centred, its neighbours cut off at the
  edges, with `anim::Slide` easing between positions;
- `zoom`: fit to width, contain, zoom about the pointer;
- `scroll::scrollbar`, `geom` (rect helpers, image crop) and `text` (clipped
  names, `x–y of N`, path segments).

Mouser's commander mode is built on it. Nothing here talks to Mouser: a mode
keeps its own state and sends the items in its panel.

```toml
[dependencies]
mouser-views = "0.1"
```

See [Building a mode plugin §Views and media](https://github.com/WeirdBrains/mouser/blob/main/docs/building-modes.md#views-and-media).
License: BSD-3-Clause.
