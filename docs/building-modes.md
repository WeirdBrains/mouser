# Building a mode plugin

This guide walks through writing a mode that docks above the shell the same
way commander, chat and preferences do. The wire format, host calls and
crash behaviour are specified in [Mode plugin protocol](./mode-protocol.md);
[Plugins](./plugins.md) covers manifests, installs and permissions. Read this
page to get a first plugin running, then keep the protocol open as the
reference.

Mouser 1.4.0 freezes **api 1**. The bundled commander, chat and preferences
plugins run on it, and so can yours.

## What you are building

A mode is its own executable. Mouser starts it the first time the mode is
used and keeps it running. stdin/stdout carry framed MessagePack (or JSON);
stderr goes to Mouser's plugin log. The plugin runs as you, with your rights
— it is not a sandbox. Every request and effect is checked against the
permissions you granted.

Two paths exist:

| path | when to use | transport |
|---|---|---|
| **Mode protocol** (`mouser-mode-api`) | a real dock with views, images, host calls, context menus | `transport = "mode"` |
| **Command mode** (one-shot JSON) | a tiny demo or shell script that returns title/lines | `transport = "command"` |

The rest of this guide is the mode-protocol path. The tree also ships
`examples/plugins/hello-mode` for the one-shot command path.

## Minimal `plugin.toml`

```toml
name = "hello"
version = "0.1.0"
permissions = ["files.read"]

[[provides]]
kind = "mode"

[run]
transport = "mode"
command = ["./hello-mode"]   # path relative to the plugin directory

[mode]
hotkey = "h"
keys = [{ id = "refresh", key = "r", describe = "refresh" }]
```

Install and allow it:

```sh
mouser plugin install ./hello
mouser plugin permissions hello run=allow files.read=allow
```

It joins the mode switcher (`esc :h`). Mouser starts it the first time the
mode is drawn. A bound key or a click arrives as `ui.action`, typing as
`input.text` while a field has focus, and the shell's folder as
`shell.cwd_changed` (with `files.read`).

## Writing it in Rust

Depend on the public crate and implement `Mode`, then call `serve` from
`main`. Only `hello` is required; the other methods have defaults.

```toml
# Cargo.toml
[dependencies]
mouser-mode-api = "0.3"
serde_json = "1"
```

```rust
use mouser_mode_api::events::{Event, Listener};
use mouser_mode_api::protocol::{ApiRange, Handled, Hello, Provide};
use mouser_mode_api::{Host, Mode, serve};
use serde_json::json;

struct Hello;

impl Mode for Hello {
    fn hello(&self) -> Hello {
        Hello {
            api: ApiRange::CURRENT,
            name: "hello".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            provides: vec![Provide::Mode {
                hotkey: 'h',
                bindings: vec![],
                activate_on: vec![],
            }],
            listeners: ["ui.action", "shell.cwd_changed", "mode.activated"]
                .iter()
                .map(|e| Listener::on(e))
                .collect(),
            emits: vec![],
            wants_mcp: false,
            shm: false,
        }
    }

    fn event(&mut self, e: &Event, host: &Host) -> Handled {
        match e.name.as_str() {
            "mode.activated" | "shell.cwd_changed" | "ui.action" => {
                let cwd = e.payload.get("cwd").and_then(|v| v.as_str()).unwrap_or("?");
                let _ = host.view(
                    Some(format!("hello — {cwd}")),
                    Some(vec![format!("event: {}", e.name), "press r to refresh".into()]),
                    vec![],
                    true,
                );
            }
            _ => {}
        }
        Handled::Handled
    }
}

fn main() {
    let _ = serve(Hello);
}
```

`crates/mouser-mode-api/src/bin/echo.rs` is a complete small example used by
the crate's tests: it answers `frame.tick` with a 1080p frame, echoes custom
events and restores state after a crash.

Build the binary, put it next to `plugin.toml` as `./hello-mode` (or change
`command`), then install as above.

### JSON instead of MessagePack

Set `MOUSER_MODE_WIRE=json` to speak JSON frames. Useful while debugging from
another language; MessagePack stays the default. In MessagePack, Mouser sends
structs as maps keyed by field name (see docs/mode-protocol.md §Framing), so
ignore unknown keys: fields are added without bumping the api.

## What arrives and what you send

**In (events):** `mode.activated` / `mode.deactivated`, `ui.action` (a bound
key or a click), `input.text` / `input.key` / `input.escape` while you asked
for the keyboard, `shell.cwd_changed`, `shell.remote_*` with `remote.read`,
`menu.open` for context menus, `view.resized` (including dock size and
`reduce_motion`),
`frame.tick` when you draw video, `agent.state` when you run an agent session.

**Out:** `View` (dock / panel / images), `Effect` (type into the shell, `cd`,
open a URL, a status line), `Request` (host calls), `Emit`, `Menu`,
`StateSave`, `Wants` (what the keyboard should give you right now).

The full tables live in [Mode plugin protocol](./mode-protocol.md) §§Events,
Messages from the plugin, Host calls, Images, Host services.

## Host calls you will use first

Declare every permission in `plugin.toml` and grant it before the call works:

| call | permission | use |
|---|---|---|
| `prefs.schema` / `prefs.get` | none | read preferences (secrets only say they are set) |
| `prefs.set` / `prefs.reset` | `prefs.write` | write non-secret prefs |
| `prefs.secret_prompt` | `prefs.write` | Mouser's popup saves a key to the keychain; you never see it |
| `Fs` read / list / stat | `files.read` | local files, never the secrets deny-list |
| `fetch` | `net.fetch` | one HTTP(S) request made by Mouser |
| `remote.*` | `remote.read` | files on the shell's ssh host |
| `agent.*` | `agent.use` | chat with the user's models and keys inside Mouser |
| `Approve` / `permissions.request` | none | Mouser's own popup |

Mouser's popups name your plugin, take every key while open, and ignore
clicks for the first 400 ms so nothing in the tab can answer them for you.

## Views and media

Two crates in Mouser's repository give a mode the views and media commander
and chat are built on. Both only build panel items and pixels for the
protocol above: nothing extra goes over the wire, and nothing in them talks to
Mouser by itself.

```toml
mouser-views = { git = "https://github.com/WeirdBrains/mouser" }
mouser-media = { git = "https://github.com/WeirdBrains/mouser" }
```

**mouser-views** lays out your items: `Layout` (list, grid or carousel, with
`next()` and the names commander uses), `grid::Grid` (tiles in rows that
scroll to the selection), `carousel::Strip` (the selection centred, neighbours
cut off at the edges) with `anim::Slide` easing between positions, `zoom`
(fit to width, contain, zoom about the pointer), `scroll::scrollbar` and the
labels in `text` (`clip_text`, `range_label`, `path_segments`).

**mouser-media** plays pictures, video and sound: `detect::find` finds media
named in text (Markdown images and links, bare paths), `player::Player` plays
video and audio with ffmpeg and cpal (output from `[audio]` in
preferences.toml), `still::Stills` decodes pictures off the UI thread, and
`anim::FsAnim` is the 220 ms ease-in-out flight to fullscreen and back.
`view::MediaView` puts these together: `inline(...)` draws a player under a
message, `fullscreen(...)` draws the open one over your space, and you pass
it the `media_*` actions from `ui.action` (`action`) and keys while it is
open (`key`). Send its pictures as images (shared memory when the ring is
there) and free a slot only after `frame.presented`.

A player draws from its own thread, so give it a `Waker` that makes your mode
draw again: emit an event of your own to yourself (chat emits `chat.tick`, at
most once per 8 ms; Mouser allows 200 emits a second) and redraw when it
arrives. Call `mouser_media::set_home` if your mode keeps its own idea of
Mouser's home (tests), so the player reads `[audio]` from the same place.

Reading a picture or clip from disk needs `files.read` in `plugin.toml`; ffmpeg
and ffprobe are found on `PATH`, and without them video shows a hint instead
of playing.

## Context menus and MCP

Answer `menu.open` with the actions that apply to the item under the pointer
(or the selection, when the menu key `.` was used). Build the list from the
same registry as your key bindings. Mouser draws the menu and turns a pick
into `ui.action`.

The same answer also feeds Mouser's MCP tools `mode_actions` /
`mode_action`, so anything on your menu is scriptable once you answer
`menu.open`. Keep destructive work behind your own confirmation; Mouser asks
the user once (`modes.control`) as well. See the protocol §Over MCP and
[MCP §Mode actions](./mcp.md#mode-actions).

## Crashes and restarts

Mouser restarts a crashed mode plugin with backoff (1 s, 2 s, 4 s) up to
`plugins.max_restarts` (default 3), then shows a Restart action. Whatever you
last sent with `StateSave` comes back in `Welcome.restore`. The terminal is
never taken down by a plugin crash.

## Checklist before you ship

1. `plugin.toml` name matches `Hello.name`, and `command` finds the binary.
2. Every host call's permission is listed and granted.
3. Destructive menu entries confirm in the plugin **and** survive an MCP
   client picking them.
4. `Wants` is cleared when a field closes, so keys return to the shell.
5. You tested a crash: kill the process and confirm it restores cleanly.

## See also

- [Mode plugin protocol](./mode-protocol.md) — the normative reference
- [Plugins](./plugins.md) — installs, permissions, other transports
- [Modes](./modes.md) — how the dock, focus toggle and menus feel to a user
- [Using Mouser's MCP server from a plugin](./mcp-from-plugins.md)
