# Preferences mode

`esc :p` opens the preferences mode (a mode plugin, `mouser-preferences`, like
commander and chat; `mouser plugin disable preferences` turns it off). It lists every
setting in `$MOUSER_HOME/preferences.toml` by section: Appearance, Window,
Keys, Plugins, Chat, Models, Local model and MCP server. Plugins has
`plugins.max_restarts`: how many crashes within 30 s a plugin may have before
it stays off (default 3; 0 never restarts; see [Mode plugin protocol](./mode-protocol.md)).

Keys while the preferences mode has the keyboard (switching to it with `esc :p` gives it; ctrl+alt+k or Esc gives it back, docs/modes.md §Focus; remap them in `modes.toml` under `[preferences.keys]`):

- ↑ / ↓ move, PgUp / PgDn page;
- ← / → change the value: the previous/next choice, on/off, or ±1 for numbers;
- Enter edits: on/off toggles, choices open a dropdown (↑/↓, Enter picks,
  Esc closes), other fields take typed text (Enter saves, Esc cancels);
- r resets the row to its default (the key is removed from the file).

Rows are clickable: the value edits it, `reset` resets it.

Every value is validated before it is saved (ranges, choices, http(s) URLs),
and the file is only written when Mouser can still read it: the change goes
through `toml_edit`, so comments, ordering and keys Mouser doesn't know are
kept, and it is written atomically (temp file + rename). A preferences.toml that
doesn't parse is never overwritten; the mode says so instead.

Theme, light/dark, the Esc wait, chat and model settings apply at once; rows
marked "new windows" (font, sidebar, MCP server) apply to windows opened later.

Secrets (the local model server key) go only to the OS keychain; the file
keeps a `keychain:NAME` reference and the mode shows "set (keychain)". With no
keychain available the secret is not saved. The model folder is shown but
moved with `mouser model local move <dir>`.

## As a plugin program

`plugins/preferences` is the mode's program (`mouser-preferences`,
`transport = "mode"`), built only on `mouser-mode-api`; Mouser has no built-in
preferences mode since 1.4.0. To run your own build instead of the official one:

```
cargo build --release -p mouser-preferences
mkdir -p /tmp/prefs-plugin && cp plugins/preferences/plugin.toml target/release/mouser-preferences /tmp/prefs-plugin/
mouser plugin install /tmp/prefs-plugin
mouser plugin permissions preferences run=allow prefs.write=allow
```

It reads and writes through Mouser's `prefs.*` calls, so Mouser still does the
validation and the comment-keeping write. For a secret, Enter makes Mouser
show its own key popup. The key you type goes from that popup to the keychain,
and the plugin only hears that it was saved.

## Context menu

Right-click a setting (or `.` while preferences has the keyboard): edit (Enter),
previous / next value (← / →, for on/off, choices and numbers), reset to default
(`r`, greyed when nothing is set), copy value (never for a key; those stay in the
keychain), copy the setting's name, previous / next setting. Off the settings:
previous / next setting and page up / down.

## Mode panel size

Window → *Mode panel height* (`window.dock_height`, a percentage of the terminal, 10–100; `auto` = the mode's own height) and *Mode panel fullscreen* (`window.dock_full`). The panel's `F`, `]`, `[` keys and dragging its bottom edge write the same two settings (docs/modes.md §Panel size and fullscreen).

## Audio

Where the player's sound goes (`[audio]` in preferences.toml; applies to the next
clip you open).

| Key | Values | Default |
| --- | --- | --- |
| `audio.host` | `auto`, `jack`, `alsa` (Linux); `auto` elsewhere | `auto` |
| `audio.output_device` | `default` or a device of that host (the dropdown lists them) | `default` |
| Active host | read-only: the host `audio.host` resolves to right now | |

- **auto** uses JACK when a JACK server answers (PipeWire's JACK, `pipewire-jack`,
  counts), else ALSA. macOS always uses CoreAudio and Windows WASAPI.
- **jack** also falls back to ALSA when no JACK server is reachable, so sound never
  just stops.
- A device that is gone (unplugged, renamed) falls back to the host's default; the
  player says `audio device "NAME" not found; using the default`. The player's status
  shows the host playing, e.g. `playing · JACK`.
- **Building with JACK.** JACK is the optional `jack` cargo feature
  (`--features mouser-app/jack,mouser-commander/jack`); building it needs the JACK
  headers and `jack.pc` (Debian/Ubuntu `libjack-jackd2-dev`, Arch `pipewire-jack` or
  `jack2`). libjack itself is loaded at run time, so a JACK build still starts and
  plays through ALSA on a machine without it. The official Linux builds include it.
  Without the feature, `jack` plays through ALSA and Active host says
  `ALSA (JACK not in this build)`.
