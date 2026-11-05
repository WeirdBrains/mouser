# Mode plugin protocol

Commander, chat and preferences are moving out of Mouser into plugin
executables built on the public `mouser-mode-api` crate
(`crates/mouser-mode-api`), the same API any third-party mode uses. This page
describes that protocol. It is **api 0**: still changing. The shipped modes are
still compiled in until Mouser 1.4.0, but you can install your own mode plugin
on it now (below).

## Process model

- A mode is its own executable. Mouser starts it once, the first time the mode
  is used, and keeps it running. stdin and stdout carry the protocol; stderr
  goes to Mouser's plugin log.
- The plugin runs as you, with your rights. It is not a sandbox. Mouser checks
  every request and effect against the permissions the plugin was granted.

## Installing a mode plugin

```toml
# plugin.toml
name = "hello"
version = "0.1.0"
permissions = ["files.read"]     # shell.type to type into the shell, ...

[[provides]]
kind = "mode"

[run]
transport = "mode"
command = ["./hello-mode"]       # ./ is the plugin's directory

[mode]
hotkey = "h"
keys = [{ id = "refresh", key = "r", describe = "refresh" }]
```

`mouser plugin install DIR`, then allow it to run with
`mouser plugin permissions hello run=allow` (and any permission it lists). It
joins the mode switcher (`esc :h`). Mouser starts it the first time it's drawn.
A bound key or a click arrives as a `ui.action` event, typing as `input.text`,
switching modes as `mode.activated`/`mode.deactivated`, and the shell's folder
as `shell.cwd_changed` (with `files.read`). An effect it isn't allowed shows a
status message instead. Its emits come back to its own listeners. Typing
reaches the plugin as `input.text` only while its view has a focused `Field`
item or an open dropdown (a `Popup` item); then bound keys don't fire; otherwise keys go to its bindings or the
shell.

Host calls (`Request`) available so far. Each needs the permission shown,
declared in `plugin.toml` and granted:

| call | permission | answer |
|---|---|---|
| `prefs.schema` | none | every preference: `section`, `key`, `label`, `default`, `live`, `kind` (`{"type": "bool"}`, `int`/`float` with `min`/`max`, `choice` with `values`, `text`, `secret`, `readonly` with `hint`) |
| `prefs.get {key}` | none | the value, or `null` for the default. A secret only says `"set (keychain)"` |
| `prefs.set {key, value}`, `prefs.reset {key}` | `prefs.write` | written to preferences.toml with its comments kept. Secrets are refused |
| `prefs.secret_prompt {key}` | `prefs.write` | Mouser's own popup asks you for the key (masked) and saves it to the keychain. The plugin gets `{"saved": true}` or `cancelled`, never the value |
| `permissions.granted` | none | the granted permissions |
| `permissions.request {permission, why}` | none | Mouser's popup asks you to grant a permission the manifest declares (Allow always / Deny / Never); `{"granted": bool}` |
| `Approve {title, detail}` | none | Mouser's popup with your own question; `{"decision": "once"}` or `"deny"` |
| `Fs` `Read`/`Stat`/`List` | `files.read` | local files, absolute paths, 1 MiB per read, never in the secrets deny-list |
| `fetch {url, method, headers, body, max_bytes}` | `net.fetch` | one HTTP(S) request made by Mouser: `status`, `headers`, `text` (or `bytes`), `truncated`. 4 MiB by default, 16 MiB at most |
| `remote.location`, `remote.list {path}`, `remote.read {path, offset, len}` | `remote.read` | the shell's ssh host, over its connection (1 MiB per read); `shell.remote_changed` events too |
| `agent.models`, `agent.set_default {model}` | `agent.use` | providers, their models and the default. Never a key |
| `agent.send {text}`, `agent.cancel`, `agent.clear`, `agent.state` | `agent.use` | an agent session that runs inside Mouser with your models, keys and tools. Replies stream to the plugin as `agent.state` events: `entries` (`user`, `assistant`, `tool`, `note`, `error`), `running`, `model`, `card_open`. Tool-call cards are drawn by Mouser, never by the plugin |

Mouser's popups name the plugin that asks. They take every key and click while
open, and their buttons do nothing for the first 400 ms, so a plugin (or the
program in the tab) can neither draw nor answer them. Slow calls (`fetch`,
`remote.*`) run on a worker thread and answer when done.

Others (changing files, spawn, timers, the clipboard) answer "not available
yet" for now. A plugin may still start its own processes.

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
