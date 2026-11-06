# Integrating an app with Fuzzball in Mouser

Fuzzball is the terminal protocol Mouser prefers for graphics, structured
keyboard events, plugins and silent shell actions. Mouser speaks it through
the [`fuzzball`](https://crates.io/crates/fuzzball) crate; Kitty graphics and
Kitty keyboard remain fallbacks when a program does not speak Fuzzball.

This guide is for **application authors** who want their TUI or CLI to look
and feel right inside Mouser (and other Fuzzball hosts). The normative spec
lives with the crate; Mouser's terminal-side behaviour is summarised in
[Fuzzball protocol](./protocol/README.md).

## Where the spec lives

| resource | URL |
|---|---|
| Crate | https://crates.io/crates/fuzzball |
| Spec site | https://weirdbrains.github.io/fuzzball/ |
| Repo docs | https://github.com/WeirdBrains/fuzzball/tree/main/docs |

Start with the crate's `docs/README.md` (framing, capabilities, versioning,
errors), then:

- `graphics.md` — upload, place, delete, query images
- `keyboard.md` — structured key events
- `plugins.md` — in-terminal plugin handshake (`p h` / `p r`)
- `shell.md` — silent shell actions
- `wire-format.md` — exact bytes on the wire

Mouser code comments that say `fuzzball docs/<file>.md §N` point at those
files. The terminal implementation is `crates/mouser-core/src/fuzzball.rs`.

## Pick a client library

In Rust, depend on `fuzzball` and let it encode and parse every APC. In
other languages, follow `wire-format.md` and speak the same APC framing
(`ESC P = F z … ESC \`). Do not invent a parallel dialect: Mouser rejects
what it does not recognise and falls back only for Kitty.

## Graphics inside Mouser

1. **Upload** image data (or a reference) with the graphics commands in the
   spec.
2. **Place** it against a cell anchor. Placements follow text through scroll
   regions, IL/DL, scrollback and resize; they disappear when their anchor
   leaves the scrollback.
3. **Query** what the host still holds; abandoned uploads expire on a timer.

Mouser-specific behaviour worth designing for:

- Resize scales placements to the screen width (`Placement::fit`); prefer
  giving a logical size and letting the host scale, rather than re-uploading
  every resize.
- `mouser show` displays an image file with Fuzzball first, Kitty as
  fallback, and Kitty Unicode placeholders inside tmux
  ([Terminal emulation](./terminal.md)).
- Layers honour z-order relative to cell backgrounds the same way Kitty
  graphics do in Mouser.

Test with a known PNG via `mouser show file.png` in a Mouser tab, then with
your own upload/place sequence. Over SSH, prefer Fuzzball's binary-safe
framing; if you must tunnel through tmux, read the crate's guidance on
placeholders.

## Keyboard mode

Request Fuzzball keyboard mode when you need distinct modifiers, release
events or layout-independent key ids. Mouser delivers structured events to
the focused program; when Fuzzball keyboard is off, Kitty keyboard (and
legacy) apply as documented in [Kitty keyboard compliance](./kitty-compliance.md).

Verify with the crate's examples or by printing events inside Mouser. Physical
key codes need a real keyboard — virtual key injectors (wtype and friends)
often report `Escape` for everything.

## In-terminal plugins over Fuzzball

A program running **in the tab** can ask Mouser to treat it as a plugin for
the life of that tab. The handshake is `p h` (host answers `p r` after its
own approval popup). Grants end when the tab closes. A `plugin.toml` with
`transport = "fuzzball"` is optional and only lists the plugin.

```sh
# From examples/plugins/hello-fuzzball — run inside a Mouser tab:
sh examples/plugins/hello-fuzzball/hello-fuzzball.sh
```

That script asks for `notify` and `screen.read`. Capabilities and the exact
APC shapes are in the fuzzball crate's `plugins.md`. This path is separate
from installed mode/MCP plugins under `$MOUSER_HOME/plugins`
([Plugins](./plugins.md)).

## Silent shell actions

Fuzzball's shell actions let a host run a small, user-visible class of
operations without typing into the shell's readline. Prefer them over
injecting keystrokes when the crate documents an action that fits. See
`shell.md` in the fuzzball docs.

## Fallback strategy

| feature | prefer | fallback in Mouser |
|---|---|---|
| Images | Fuzzball graphics | Kitty graphics (APC), then Unicode placeholders in tmux |
| Keys | Fuzzball keyboard | Kitty keyboard, then legacy |
| Plugins | Fuzzball `p h` | installed plugins via `mouser plugin install` |

Detect the host: if Fuzzball capability negotiation succeeds, use it; otherwise
degrade. Never send both Fuzzball and Kitty for the same image.

## Checklist

1. Encode through the `fuzzball` crate (or a careful port of `wire-format.md`).
2. Graphics placements survive scroll and resize without re-upload storms.
3. Keyboard mode turns off cleanly when your TUI exits.
4. Plugin handshake shows Mouser's popup and handles deny / timeout.
5. Over SSH and inside tmux, images still appear (placeholders if needed).

## See also

- [Fuzzball protocol](./protocol/README.md) — Mouser's index into the crate docs
- [Terminal emulation](./terminal.md) — `mouser show`, scrolling, sequences
- [Plugins](./plugins.md) — installed plugins vs in-tab Fuzzball grants
- [Kitty keyboard compliance](./kitty-compliance.md)
