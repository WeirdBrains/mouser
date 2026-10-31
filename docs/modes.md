# Modes

Mouser has two kinds of modes: **mode plugins** (commander, below), which change how
you work with the files under the shell, and the **input modes** of the command line.

## Input modes

Mouser supports vim-style input modes:

- Shell (default): input goes to the PTY.
- Command: `:` command line for actions (e.g., `:open-sidebar`, `:connect <name>`, `:theme <name>`).
- Config: reserved for in-app editing of preferences/themes.

Mode switching
- Enter Command: F10 by default (configurable in future). Esc returns to Shell.
- Mode indicator: the command line is shown at the bottom when in Command mode.

Actions
- `:open-sidebar` / `:close-sidebar`
- `:list-conns`, `:connect <name>`
- `:theme` — open the theme switcher; `:theme <name>` — switch theme (saved)
- `:mode dark|light|system` — switch variant mode (saved)
- `:edit-theme` — open the theme & font editor for the active theme variant
- `:font-size <6-48>`, `:font <family>` (`:font` alone resets to the default stack)

Keybindings
- F10: command line. Alt+Esc: toggle the connections sidebar.
- Ctrl+= / Ctrl++ bigger font, Ctrl+- smaller, Ctrl+0 reset to `config.toml`'s size (saved to
  `preferences.toml`; the grid reflows and the shell is resized).
- Theme switcher: ↑/↓ theme (live preview), ←/→ or d/l/s mode, e editor, Enter apply, Esc cancel.
- Editor: Tab/Shift+Tab or ↑/↓ move between fields, type to edit (live preview when valid),
  Ctrl+S save, Esc cancel. Fields are also clickable.

Menus, keybindings and the command line dispatch the same named actions.


## Mode plugins

Exactly one mode plugin is active at a time (or none). Mouser starts in the default
mode, **commander**. Switching replaces the current mode; there is no stacking.

- `esc :` then the mode's hotkey and Enter switches to it (`esc :c` = commander).
  Typing the hotkey of the mode that is already on turns modes off (plain terminal);
  `esc :c` again turns commander back on. `:c` / `:commander` work from F10 too.
- A lone Esc waits 500 ms for the `:`; without it the Esc goes to the shell
  unchanged (so `esc` keeps working in readline/vi-mode shells, just 500 ms later).
  `alt+:` opens the command line at once. Esc first closes an open mode window or
  search.
- Full-screen programs (the alternate screen: vim, less, htop ...) get the whole
  window: the mode's view is hidden and no key is taken, Esc included.
- `ctrl+shift+k`: the key mappings of the current mode. `ctrl+shift+m`: all modes
  (click one to switch).
- Every mode key is `<modifier>+<key>` (modifier `alt`). Only the keys a mode binds
  are taken; every other key, other alt keys too, goes to the shell.

### Remapping

`~/.mouser/modes.toml` (the Mouser home), one section per mode:

```toml
[commander]
modifier = "alt"        # only alt for now
[commander.keys]
switch_view = "v"       # a character, or up / down / left / right / enter
```

An unknown action or key is reported in the status bar and the defaults are kept.

### Commander

A view docked above the prompt (the terminal grid moves down; about half the
window) that follows the shell's working directory (`/proc/<pid>/cwd` on Linux,
OSC 7 elsewhere) and re-lists when the directory changes. Header: the directory,
item count and the equivalent `ls` flags, then the view, `options`, `man` and `help`
buttons; a search field filters the folder as you type.

| key | action |
| --- | --- |
| key (alt+) | list | grid | carousel |
| --- | --- | --- | --- |
| t | switch view: list → grid → carousel | | |
| ↑ / ↓ | previous / next row | row above / below (follows the wrap) | previous / next item |
| ← / → | previous / next item | previous / next tile | previous / next item |
| PgUp / PgDn | a screen of rows | a screen of tiles | 5 items |
| Home / End | first / last | first / last | first / last |
| p | parent folder (`cd ..`) | same | same |
| Enter | open: a folder is `cd`'d into, video/audio go to the desktop opener (`xdg-open`, `open` on macOS), anything else opens the full-width file view (image or text; ↑/↓/PgUp/PgDn scroll it, ←/→ step through files) | same | same |
| q (or Esc) | close the file view / window | same | same |
| s | search: type to filter (the first match is selected), Enter keeps the filter, Esc clears it | same | same |
| space | mark / unmark the item (then moves on in list and carousel) | same | same |
| * | mark all / clear marks | same | same |
| \| | pipe: a prompt in the view; Enter types `printf '%s\n' <paths> \| <cmd>` into the shell, so the output lands in the shell | same | same |
| z / x | | larger / smaller tiles | |
| a | dotfiles on/off (`ls -a`) | same | same |
| o / m | options window / manual | same | same |

**What you have typed is kept.** `cd` (open a folder, alt+p) and pipe run without
losing a half-typed command: Mouser sends Ctrl+E, a space and Ctrl+U (the line goes to
the shell's kill ring), the command and Enter, then, once the shell is the terminal's
foreground process again (the command finished), Ctrl+Y and Backspace to put the line
back (the cursor ends at its end). The added space makes sure an empty line stays empty
instead of yanking an older kill.

| shell | |
| --- | --- |
| bash (readline, emacs mode) | kept (tested) |
| zsh (zle, emacs keymap) | kept (tested) |
| fish | kept (Ctrl+U/Ctrl+Y use its kill ring; tested when installed) |
| vi keymaps (`set -o vi`, `bindkey -v`) | not supported: in insert mode Ctrl+E/Ctrl+Y differ; leave the line empty before opening |
| a program reading the terminal (`less`) after pipe | the yank waits until it exits |

On Windows the shell's foreground state cannot be read, so the line is put back after
a short pause.

Mouse in every view: a click selects, a double-click (400 ms) opens, the wheel moves the
selection (it scrolls the file view and the manual). The selection is highlighted and
the view scrolls to keep it on screen. Marks show as `*` and as a count in the header.
Pipe takes the marked paths in listing order, or the selected path when nothing is
marked; paths are single-quoted (`'` becomes `'\''`). Marks are per folder.

The defaults avoid readline's alt keys except `alt+t` (transpose-words, from
MODES.md), which commander takes while it is on. Clicks: a click selects an item, a
second click opens it; the wheel over the view moves the selection.

- **List**: `ls -lah` columns (attributes, size, name, last modified).
- **Grid**: tiles with a preview by type (images scaled, the first lines of text
  files, a label for folders, video and audio) and the name below.
- **Carousel**: the focused item large in the middle and its two neighbours cut
  off at the edges.

Previews load lazily on a worker thread: only what is on screen is requested
(grid: the visible tiles; carousel: the focused item and its two neighbours) and
the queue is replaced as you move, so off-screen items are skipped. Formats: PNG,
JPEG, GIF, WebP, BMP.

### Plugin API (`mouser_render::modes`)

A mode implements the `Mode` trait and is registered with `Host::new` (the first
mode is the default). Built-in modes are listed in `Host::builtin`.

| item | what the mode provides |
| --- | --- |
| `name()`, `hotkey()` | name and the letter typed after `esc :` |
| `bindings()` | default bindings `Binding { id, key, describe }`; the host loads `modes.toml` overrides, shows them in `ctrl+shift+k` and routes a bound key to `action(id, 0)` |
| `activate()`, `deactivate()` | lifecycle; `deactivate` drops transient state |
| `cwd_changed(dir)` | the shell's directory (called every frame; cheap when unchanged) |
| `action(id, arg)` | a bound key, or a click on an item with `UiAction::Mode { action, arg }`; the wheel over the view sends `"scroll"` with ±steps |
| `wants_text()`, `text(bytes)` | a focused input (search) takes plain typing |
| `escape()` | close the mode's window or input |
| `view(ctx, keymap)` | the render contribution: `View { dock, panel, images }` — a panel docked above the prompt inside `ctx.area`, an optional floating window and images (window pixels plus source rect) |
| `set_waker(w)` | redraw from another thread (e.g. when a preview finished) |

Actions return an `Effect` for Mouser to carry out: `Type` text into the shell,
`Open` a file with the desktop opener, `ShowKeys`, `Say` (status bar). The core
has no commander-specific code: key routing, `esc :`, the key and mode windows and
remapping are generic.

The interface passes plain data only (ids, paths, panels, pixels), so it can be
served across a C ABI or a socket later.

**Future work:** third-party modes, loaded like engines (a shared library with a
versioned ABI or a helper process speaking the same calls), installed under
`~/.mouser/modes/`; more modifiers than `alt`; DJ, Director, Debugger and Artist
from MODES.md.
