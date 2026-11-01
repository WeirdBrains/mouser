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
  `alt+:` opens the command line at once. A line whose first word is not a Mouser
  command runs in the tab's shell (typed like commander's cd: what you had typed is
  kept and comes back), so `echo "hello, world!"` just works; in an SSH tab it runs
  remotely. Mouser commands win on a name clash; a leading `!` forces the shell
  (`!theme` runs a program called theme). If another program owns the terminal the
  line is not typed and a notice says so. Esc first closes an open mode window or
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
| p, Backspace | back: parent folder (`cd ..`); in the file view / player it closes the view | same | same |
| Enter | open: a folder is `cd`'d into, video/audio open the player (below), anything else opens the full-width file view (image or text; ↑/↓/PgUp/PgDn scroll it, ←/→ step through files) | same | same |
| q (or Esc) | close the file view / window | same | same |
| s | search: type to filter (the first match is selected), Enter keeps the filter, Esc clears it | same | same |
| space | mark / unmark the item (then moves on in list and carousel) | same | same |
| * | mark all / clear marks | same | same |
| \| | pipe: a prompt in the view; Enter types `printf '%s\n' <paths> \| <cmd>` into the shell, so the output lands in the shell | same | same |
| z / x | | larger / smaller tiles | |
| a | dotfiles on/off (`ls -a`) | same | same |
| o / m | options window / manual | same | same |

**Player.** Video and audio open in commander's own player (MODES.md: "a player will
appear"). Video frames are decoded by `ffmpeg` to RGBA, fitted to the view's width and
drawn through Mouser's image path; Mouser plays the sound itself (cpal: ALSA/PipeWire on
Linux, CoreAudio, WASAPI). Audio files get a minimal view: name, progress bar, time,
volume and the keys. Keys (remappable, they only act while a player is open; otherwise
they go to the shell):

| key | |
| --- | --- |
| alt+k | play / pause |
| alt+j / alt+l | back / forward 10 s |
| alt+- / alt+= | volume down / up (10 %) |
| alt+q or Esc | close |

How it stays in sync: two `ffmpeg` readers of the same file start at the same seek point,
one for video (`-an`, RGBA at the file's frame rate, each frame stamped with its PTS =
start + n/fps) and one for audio (`-vn`, f32 at the device rate). The audio output
callback counts the samples it hands to the device and notes when each buffer was
handed over; the clock is the frame being played now: that buffer's first frame plus the
time since, minus the device latency the backend reports (so it moves smoothly instead of
in buffer-sized steps), never past what the device was given. Frames can only appear
when the window redraws, so the clock is read half a redraw interval ahead (measured as
a moving average): the error is centred on zero. A due frame is shown unless it is
already a whole frame late and the next one is due too; only then is it dropped; early
frames are held. Files without audio use a wall clock. Pause silences the output and
freezes the clock (the readers just wait, nothing restarts); volume is applied to the
samples in-process. A seek flushes both queues and restarts both readers with `-ss` at
the target, so audio and video start together. Measured (Linux, 25 fps test file with a
flash and a beep each second, 64 s of play plus 7 seeks, 2350 frames): video vs the
audio clock mean -0.2 ms, p95 9.5 ms, max 24.7 ms, 0 frames dropped, no drift
(first 10 s +0.5 ms, last 10 s -2.9 ms); 61-76 ms from a seek key to the first new frame.
`MOUSER_PLAYER_LOG=<file>` writes `clock pts` per shown frame and the seek gaps.

Nothing is started until a file is opened; playback stops (the readers are killed) when
the viewer closes, another file is shown or the mode is left. Without `ffmpeg` and
`ffprobe` on PATH, media falls back to the desktop opener (`xdg-open`, `open` on macOS).

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

`.` and `..` are never listed. Going up is the **back** control in the header
(`‹ back (alt+p)`, `‹ back` when narrow; greyed out at `/`), `alt+p` or
`alt+Backspace` (both remappable, both in `ctrl+shift+k`; alt+Backspace shadows
readline's backward-kill-word while commander is on). Going up selects the folder you
came from. The header path is clickable: each segment goes to that folder (long paths
lose their leading segments to `…`).

Search (`alt+s` or a click on the field) is the only thing that takes plain keys,
and only while it is focused (bright field, "searching · type to filter" hint). It
filters as you type and shows `N matches for "q"` or `no matches for "q"`; Enter
keeps the filter, Esc cancels. Everything else you type, viewer or player open or not,
goes straight to the shell. An Esc right after an Esc that closed a viewer or window
(within 1.5 s) is swallowed instead of being held for the shell: held, it reached the
shell glued to the next key (Esc c is readline's meta-c), which ate the first letter of
what you typed next.

- **List**: `ls -lah` columns (attributes, size, name, last modified).
- **Grid**: tiles with a preview by type (images scaled, a frame of each video, the
  first lines of text files, a label for folders and audio) and the name below.
- **Carousel**: the focused item large in the middle and its two neighbours cut
  off at the edges.

Previews load lazily on a worker thread: only what is on screen is requested
(grid: the visible tiles; carousel: the focused item and its two neighbours) and
the queue is replaced as you move, so off-screen items are skipped. They load in
order: grid tiles top-left first, the carousel's focused item before its
neighbours. Until one is ready the tile shows its kind dimly (`image`, `▶ video`).
Formats: PNG, JPEG, GIF, WebP, BMP. Video thumbnails: one `ffmpeg` frame at 10 % of
the duration (at most 30 s in; 1 s if unknown; the first frame if that fails), scaled
to the tile, cached in memory and on disk in `$MOUSER_HOME/cache/thumbs/` keyed by
path, mtime, size and tile size. Without ffmpeg the tile says `▶ video`.

Previews and player frames are clipped to the view (never over the prompt), and are
left out under the options and manual windows so nothing shows through them.

**Carousel motion.** Moving between items slides the strip (the focused item and the
partial neighbours together) over 180 ms with an ease-in-out curve (cubic; ease-in alone
looked like it lurched at the stop). A key during a slide retargets from where the strip
is, without a jump; jumps of more than 3 items only slide the last one. Redraws are
requested only while it moves (a ticker every 8 ms, stopped at the end). Measured
(Linux, release, 655 px window): 50 fps while sliding (frame p50 19.8 ms, p95 20.0,
max 38.6), idle frames only on other events. Loading stays lazy: what is on the strip
plus the selection's neighbours, so the incoming item loads during the slide.

**SSH sessions.** When the tab's shell is on another host commander lists that host:

- Remote when the terminal's foreground process is `ssh` (typed, or the sidebar's
  launcher), or when an OSC 7 `file://host/path` names a host other than this one while
  the local shell is not in the foreground. Folder: the remote shell's OSC 7 (the
  remote shell must send it, e.g. `PROMPT_COMMAND='printf "\033]7;file://%s%s\007"
  "$HOSTNAME" "$PWD"'`).
- Listing: `ls -la` on the host through a side channel: `ssh` with the running
  command's own connection options (`-p -l -i -F -J -o -4 -6`; `-F $MOUSER_SSH_CONFIG`
  if the command has none), `BatchMode=yes` (never prompts), multiplexed with
  `ControlMaster=auto`, `ControlPath=$MOUSER_HOME/cache/ssh/%C`, `ControlPersist=120`.
  Fetched on a thread, refreshed every 5 s while shown.
- Header: `user@host:/path` (clickable segments, back works); `cd`, back and pipe are
  typed into the PTY, so they run remotely.
- Previews are not fetched: tiles show their kind with a `· remote` badge. Opening a
  remote file (viewer or player) is not supported: a notice says so (pipe it into a
  command instead).
- No OSC 7 yet: `remote (host): no directory info ...` with a hint; no side channel
  (mosh, a nested ssh): `remote: no ssh side channel ...`; the side channel failing
  (password-only host): ssh's error and a hint. Local files are never shown for a
  remote shell. After `exit` the local shell is back and so is its folder.

#### Image zoom, player controls, header menu

- Image view: `alt+=` / `alt+-` zoom (they are the player's volume keys too: in the image view they zoom, in the player they change volume; remap `zoom_in`/`zoom_out` to split them), `alt+0` fit, `alt+1` 100%. Ctrl+wheel zooms about the pointer; when zoomed, drag, `alt+arrows` or the wheel pan. The header shows `fit 37%` / `200%`. Past 2048 px the full image is decoded (the preview shows until it is ready). Images stay inside the view and clear of its rounded corners (the renderers have no per-pixel clip, so images are inset rather than masked).
- Player: `« 10s`, play/pause and `10s »` buttons; the seek bar seeks on click, scrubs on drag (seeks at most every 120 ms, a final one on release) and shows the hovered time; the volume bar takes click, drag and wheel. Clicking the video toggles play.
- Header: when narrow, the path is elided first (down to ~16 columns), then the buttons that still do not fit go, one by one from the left, into a `•••` menu (`menu`, default `alt+i`).
- Boxes: 8 px rounded corners and a three-layer soft shadow (stronger on dark themes, faint on light), built from rects so every renderer draws them.

#### Future: remote fetch layer / sync

Planned (t185u/t186u), not built yet: ranged parallel fetch of remote files into a sparse cache (range map, mtime+size key, rsync/scp completion, transfer queue) behind its own module so a later local<->remote sync feature can reuse it.

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
