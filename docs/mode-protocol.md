# Mode plugin protocol

Commander, chat and preferences are moving out of Mouser into plugin
executables built on the public `mouser-mode-api` crate
(`crates/mouser-mode-api`), the same API any third-party mode uses. This page
describes that protocol. It is **api 0**: still changing, and not yet used by
the shipped modes, which are compiled in until Mouser 1.4.0.

## Process model

- A mode is its own executable. Mouser starts it once, the first time the mode
  is used, and keeps it running. stdin and stdout carry the protocol; stderr
  goes to Mouser's plugin log.
- The plugin runs as you, with your rights. It is not a sandbox. Mouser checks
  every request and effect against the permissions the plugin was granted.

## Writing a plugin in Rust

Implement `mouser_mode_api::Mode` and call `serve` from `main`. Only `hello`
is required; `welcome`, `event`, `response` and `stopping` have defaults.
`serve` sends the `Hello`, waits for the `Welcome`, maps the shared-memory
ring and hands every event to your mode on one thread, in order. The `Host`
handle it passes in sends views, effects, emits and host calls, and can be
cloned into worker threads. A worker that emits an event gets it back
through the mode's own listener, in order with input. A sync listener's
`Handled`/`Pass` answer is sent back for you.

Panels use the mode's own `Action`s (`None`, or `Mode {action, arg}`); a
click on one comes back as a `ui.action` event. Mouser converts them into
its own action type, so a plugin can't trigger Mouser's windows or
approvals.

`crates/mouser-mode-api/src/bin/echo.rs` is a complete small example.

## Framing

Each message is a 4-byte little-endian length followed by the message body.
The body is MessagePack by default. Set `MOUSER_MODE_WIRE=json` to use JSON
instead, which makes plugins easy to write and debug in any language. Frames
over 64 MiB are refused.

## Handshake and versions

1. The plugin sends `Hello`: the api range it speaks (`min`, `max`), its name
   and version, what it provides (a mode with its hotkey, key bindings and the
   events that may activate it, or a background service), the **listeners** it
   registers, the custom events it **emits** (all named `<plugin>.*`), whether
   it wants its own MCP connection, and whether it can use shared memory.
2. Mouser replies `Welcome::Accepted` with the chosen api (the newest version
   both sides speak), a session id, the drawing area, the granted permissions,
   the listeners it refused and why (not fatal), the shared-memory ring, the
   plugin's MCP grant, and the state it saved before a crash. If no version is
   shared it replies `Welcome::Rejected` with a reason ("update the plugin" or
   "update Mouser"), the plugin exits, and the mode is shown as incompatible.
   The terminal is never affected.

Mouser accepts the current api and the previous one. The api number changes
when the wire format breaks, separately from the crate's version. Plugins can
add or drop listeners later with `Subscribe` and `Unsubscribe`.

## Events

Everything Mouser tells a plugin is an `Event`: a sequence number, a time,
the tab, a name such as `shell.cwd_changed`, `input.key`, `ui.action` or
`frame.tick`, a payload, and where it came from. A listener names one event
or a family (`input.*`) and can be async (the default), sync (answers
`Handled` or `Pass` within 16 ms, at most 50 ms) or a responder.

Plugins can emit their own declared events. Emits go through the same
dispatcher as Mouser's events, so the emitting plugin hears them too (unless a
listener opts out), in order with everything else. Some limits apply:

- A plugin can never emit facts only Mouser knows (`shell.*`, `term.*`,
  `agent.*`, `approval.*`, `tab.*` and similar).
- It can synthesize input (`input.key`, `ui.action` ...) only into its own
  mode, unless you grant `events.inject:<other>`. Synthetic input never
  reaches the shell.
- A chain of emits stops after 8 hops, one emit reaches at most 64 listeners,
  and each plugin may emit 200 events a second (bursts of 1000).

## Messages from the plugin

`View` (dock, floating panel, images, fullscreen; the latest wins), `Effect`
(type or run in the shell, `cd`, open a file or link, show keys, a status
message, or pass the key on), `Frame` (a video frame already in shared
memory), `Emit` and `EmitRequest`, `Respond`, `Request` (a host call),
`Reply` (for sync listeners), `StateSave` (handed back after a restart),
`Subscribe`, `Unsubscribe`, `Log` and `Wake`.

## Host calls

`Request` carries a call, answered by exactly one `Response`:

- **Files:** read, write (atomic if asked), append, stat, list, mkdir,
  remove (to the trash by default; deleting for good always asks), rename,
  chmod and watch, on the local machine or a remote tab over its SSH
  connection.
- **Processes:** spawn with pipes, a private pty, or typed into a terminal tab
  you can see; stdin, kill and resize.
- **Escalation:** a plugin never gets more than your own rights. To run
  something as root it asks with `Escalate`; Mouser shows a card every time
  (there's no "always"), then types the command into your visible terminal so
  sudo asks you itself, or shows the Windows UAC or macOS system prompt.
  Mouser never sees a password. `sudo`, `doas`, `pkexec`, `su` and `run0` are
  refused anywhere else.
- **Services:** preferences, models and the agent, approvals, timers and the
  clipboard (see below).

`~/.ssh`, `~/.gnupg`, keychains and Mouser's secrets and config are off limits
unless you allow them.

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
| macOS (Apple silicon), shared memory | 0 of 600 | 0.8 / 1.6 ms | 0.9 ms |
| macOS (Apple silicon), inline | 0 of 600 | 3.5 / 4.7 ms | 0.5 ms |

A key press takes 8 µs (Linux), 26 µs (Windows) or 5 µs (macOS) to reach the
plugin and come back. Run the benchmark with
`cargo test -p mouser-mode-api --release -- --ignored`.

## Host services

A plugin calls Mouser's services with a `Service` call. Each method needs a
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
again after 1 s, then 2 s, then 4 s, and hands it the state it saved. After `max_restarts` crashes within
30 seconds (a preference, 3 by default; 0 never restarts) Mouser stops
trying. The plugin stays off, and the status bar shows it with a restart
button, which starts the count again. Set it with `plugins.max_restarts` in preferences. Your shells keep running throughout,
because they live in the session server, not in the plugin.

See also: [Modes](./modes.md), [Plugins](./plugins.md).
