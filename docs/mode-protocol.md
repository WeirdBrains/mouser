# Mode plugin protocol

Commander, chat and preferences are moving out of Mouser into plugin
executables built on the public `mouser-mode-api` crate
(`crates/mouser-mode-api`), the same API any third-party mode uses. This page
describes that protocol. It is **api 0**: still changing, and not yet used by
the shipped modes, which are compiled in until Mouser 1.3.0.

## Process model

- A mode is its own executable. Mouser starts it once, the first time the mode
  is used, and keeps it running. stdin and stdout carry the protocol; stderr
  goes to Mouser's plugin log.
- The plugin runs as you, with your rights. It is not a sandbox. Mouser checks
  every request and effect against the permissions the plugin was granted.

## Framing

Each message is a 4-byte little-endian length followed by the message body.
The body is MessagePack by default. Set `MOUSER_MODE_WIRE=json` to use JSON
instead, which makes plugins easy to write and debug in any language. Frames
over 64 MiB are refused.

## Handshake and versions

1. The plugin sends `Hello`: the api range it speaks (`min`, `max`), its name,
   version, hotkey, key bindings, the permissions it wants, and whether it can
   use shared memory.
2. Mouser replies `Welcome::Accepted` with the chosen api (the newest version
   both sides speak), the drawing area, the granted permissions and the
   shared-memory ring. If no version is shared it replies `Welcome::Rejected`
   with a reason ("update the plugin" or "update Mouser"), the plugin exits, and
   the mode is shown as incompatible. The terminal is never affected.

Mouser accepts the current api and the previous one. The api number changes
when the wire format breaks, separately from the crate's version.

## Messages

Mouser to the mode: `Activate`, `Restore`, `Deactivate`, `Action {id, arg}`
(a bound key or a click), `Text` (typing into the mode's input), `Escape`,
`CwdChanged`, `Resize`, `ThemeChanged`, `Tick` (only after the mode asked for
ticks, such as the video player), `Reply` (an answer to a request) and
`Shutdown`.

The mode to Mouser: `View` (dock, floating panel, images, fullscreen, sent only
when it changes), `Effect` (type or run in the shell, `cd`, open a file or
link, show keys, a status message, or pass the key on), `Wake`, `Log`,
`Request {id, method, params}` and `Ticks {fps}`.

## Images

Mouser creates a ring of image slots in shared memory (in `/dev/shm` on Linux,
otherwise the temp directory) and passes its path in the handshake. The plugin
writes RGBA pixels into a slot and sends only a small `ImageRef` naming the
slot. Mouser copies the slot into the GPU texture cache under the image's key,
so an image that hasn't changed is sent as `Cached` and never copied again.
Where the ring can't be mapped, pixels go inline in the message.

Measured with the test plugin (`mouser-mode-echo`), 1080p RGBA at 60 fps for
10 s:

| | frames dropped | round trip p50 / p99 | Mouser's share per frame (p99) |
|---|---|---|---|
| Linux, shared memory | 0 of 600 | 1.1 / 1.4 ms | 0.9 ms |
| Linux, inline | 3 of 600 | 5.6 / 8.7 ms | 0.6 ms |
| Windows, shared memory | 0 of 600 | 1.3 / 1.8 ms | 1.0 ms |
| Windows, inline | 0 of 600 | 7.7 / 10.3 ms | 1.6 ms |

A key press takes 8 µs (Linux) or 26 µs (Windows) to reach the plugin and come
back. macOS has not been measured yet. Run the benchmark with
`cargo test -p mouser-mode-api --release -- --ignored`.

## Host services

A plugin calls Mouser's services with `Request`. Each method needs a
permission:

| method | permission |
|---|---|
| `remote.location`, `remote.list` | `remote.read` |
| `prefs.schema`, `prefs.get` | none |
| `prefs.set` | `prefs.write` |
| `agent.models`, `agent.send`, `agent.cancel` | `agent.use` |
| `permissions.granted`, `permissions.request` | none |
| `fetch` | `net.fetch` |

Effects that type into the shell (`Type`, `Run`, `Cd`) need `shell.type`, and
`Open` needs `files.read`. Model calls go through `agent.*`, so a plugin never
sees an API key. Tools run inside Mouser, after an approval popup that Mouser
draws itself and a plugin can't cover or imitate.

## Crashes

If a plugin exits or its pipe breaks, Mouser drops its view and starts it
again after 1 s, then 2 s, then 4 s. After `max_restarts` crashes within
30 seconds (a preference, 3 by default; 0 never restarts) Mouser stops
trying. The plugin stays off, and the status bar shows it with a restart
button, which starts the count again. Your shells keep running throughout,
because they live in the session server, not in the plugin.

See also: [Modes](./modes.md), [Plugins](./plugins.md).
