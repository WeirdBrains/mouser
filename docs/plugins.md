# Plugins

Mouser plugins add modes, tools, model providers and `[mouser]` line commands.
This is phase 1 of docs/plans/mcp-agent.md: the manifest, registry, CLI,
permission broker with approval popups, and the three transports. Mouser's MCP
server (P2), the MCP client in the sidebar (P3), model providers (P4) and the chat
mode (P5) build on it. Since P6 everything is a plugin: the built-in modes
(commander, chat) and engines (mouser, skia) are compiled-in plugins with their
own `plugin.toml` (§Built-in plugins), and engines install, list and switch through
the same registry.

## Transports

| transport | what it is | lifetime | trust |
|---|---|---|---|
| `mcp` | a program speaking MCP over stdio (`run.command`) | started on demand | installed: asks, then remembers what you choose |
| `fuzzball` | a program running *in* a tab, over Fuzzball `p` (docs/protocol/plugins.md) | while it runs, that tab only | untrusted; over ssh it asks every time |
| `command` | an executable behind `[mouser]` line words: one JSON request on stdin, one JSON reply on stdout | one run per command (10 s limit) | installed: asks, then remembers |
| `http` | a model provider reached over HTTP: a `[model]` table with `api` (openai, anthropic, google), `url`, `models`, `default` (docs/models.md) | when a model is used | built in or listed by you |
| `library` | an engine: a native library over the engine C ABI, with an `[engine]` table (docs/engines.md) | loaded when it draws | installed by you; checked (name, ABI) at install and load |
| `builtin` | compiled into Mouser (commander, chat, the mouser and skia engines, the model adapters); reserved | always there | part of Mouser |

Native libraries are only for engines (kind `engine`, transport `library`); their
C ABI is unchanged (docs/engines.md).

## The manifest: `plugin.toml`

```toml
name = "hello-command"          # a-z 0-9 - _ ; not a built-in name
version = "0.1.0"
api = 1                         # optional; plugin API version (1)
description = "says hello from the [mouser] line"
permissions = ["run", "notify"] # what it may ask for (below)

[[provides]]                    # one or more
kind = "command"                # mode | engine | model-provider | tool-provider | command
id = "hello"                    # optional; defaults to the name

[run]
transport = "command"           # mcp | fuzzball | command
command = ["sh", "./hello.sh"]  # ./ is the plugin's directory; other names come from PATH
commands = ["hello"]            # command plugins only
```

A mode plugin adds `[mode]` (hotkey and keys, §Mode plugins). An engine plugin
provides only `kind = "engine"`, uses `transport = "library"` and adds

```toml
[engine]
abi = 1                         # engine C ABI version
library = "libmouser_engine_flat.so"   # a file in the plugin directory
```

The older `engine.toml` (`name`, `version`, `abi`, `library`, `description`) is
read as the same manifest, so existing engines keep working; an engine directory
may hold either. `mouser plugin install` hands an engine directory to the engine
installer (it lands in `$MOUSER_HOME/engines/<name>`).
Bringing in a model is a plugin of kind `model-provider`. The llama.cpp/Gemma
plugin (P4b) and the hosted adapters (P4) are the first of these, and anyone can
add another.

## CLI

```
mouser plugin list                       # by priority: version, transport, kinds, commands
mouser plugin install <dir|plugin.toml>  # copies the directory into $MOUSER_HOME/plugins/<name>
mouser plugin uninstall <name>           # also forgets its permissions
mouser plugin enable|disable <name>
mouser plugin priority <a> <b> ...       # the rest follow in their order
mouser plugin move <name> first|last|up|down|<n>
mouser plugin permissions                # every saved permission, by key
mouser plugin permissions <key> [cap=allow|ask|never ...]    # `*=never` for all; fuzzball:<name>, ssh:… too
mouser plugin permissions '<key>' revoke  # forget it; it asks again
mouser plugin probe <name>               # MCP: handshake and list its tools
```

`--kind mode|engine|model-provider|tool-provider|command` limits any of these to
one kind. **`mouser engine X` is `mouser plugin X --kind engine`**: the same
parser and code (`mouser engine list` and `mouser plugin list --kind engine` print
the same thing). Engines also have `use <name>` (put first, unskip) and
`unskip <name>|--all`; without `--kind`, `enable`/`disable`/`use`/`unskip` with an
engine's name reach the engine. Installing from a URL isn't supported. State lives
in `$MOUSER_HOME/plugins/`: one directory per plugin, `state.toml` (priority,
disabled; engines' disabled flags too) and `permissions.toml`. The engine order
stays in the `engines` preference (`mouser engine priority`), not in `state.toml`.

### Built-in plugins

| name | kind | hotkey | can be disabled |
|---|---|---|---|
| `commander` | mode | `c` | yes |
| `chat` | mode | `a` | yes |
| `mouser` | engine (wgpu, CPU fallback) | | **no**: the permanent last fallback |
| `skia` | engine (in builds with the `skia` feature) | | yes |
| `openai`, `xai`, `openrouter`, `moonshot`, `anthropic`, `google`, `gemma-local` | model-provider | | yes |

Their manifests are compiled in (`crates/mouser-core/src/builtin/*.toml`,
`transport = "builtin"`; no dynamic loading of Mouser's own code). Disabling
commander and chat leaves no mode: Mouser is then a plain terminal (the mode-less
fallback), and `mouser plugin list` says so. A disabled engine is passed over when
a window picks its engine and is hidden from the sidebar; `MOUSER_ENGINE=a,b`
still forces an order for one run, as given.

### Mode plugins

A mode plugin is a `plugin.toml` with `kind = "mode"`, `transport = "command"` and
a `[mode]` table (example: `examples/plugins/hello-mode`):

```toml
[mode]
hotkey = "h"                    # one letter/digit; not c, a (built-ins) or p
keys = [{ id = "time", key = "t", describe = "show the time" }]
```

It joins the mode switcher (`esc :h`; listed in `ctrl+shift+m`) after the built-in modes and
docks above the prompt like them. Mouser runs its program once per event, off the
UI thread, with the command-plugin contract (10 s limit, scrubbed environment):
`{"api":1,"command":"mode","args":[EVENT,ID],"cwd":...}` with EVENT `activate`,
`key` (ID = a `[[mode.keys]]` id, remappable in `modes.toml`) or `cwd`; the reply
`{"title":..., "lines":[...], "text":...}` sets the dock (40 lines at most) and its
status. It runs only once `run` is allowed (`mouser plugin permissions NAME
run=allow`; until then the dock says how), and the shell's directory is sent only
with `files.read` allowed. A mode plugin shows text and reacts to keys; it can't type
into the shell or read the screen. Mode plugins over MCP are not supported yet
(the manifest check says so); two plugins with the same hotkey: the later one is
left out with a notice.

## Permissions

| capability | meaning |
|---|---|
| `run` | run its program on this computer |
| `notify` | show notices |
| `screen.read` | read the visible text of the tab |
| `scrollback.read` | read the tab's scrollback |
| `shell.type` | type or run commands in a shell |
| `files.read` / `files.write` | read / change or delete files |
| `network` | use the network |
| `models.download` | download or start a model |

Each plugin and capability is `allow`, `ask` (the default) or `never`. When a
plugin needs a capability, the broker checks it:

- any `never`: refused;
- everything `allow`: goes ahead;
- otherwise: the **approval popup**.

The popup offers **Allow once [y]**, **Always [a]**, **Deny [n]/Esc** and
**Never [N]**. Always and Never are saved.

### The approval popup can't be spoofed
- Mouser draws it as its own panel above the terminal, never as terminal text.
  Programs only write into the grid, which it covers.
- While it is open, it takes every key, paste and click. Nothing reaches the
  tab, the mode or the `[mouser]` line, so a program can't answer it.
- Buttons and keys do nothing for the first 400 ms (no click-through). Esc
  (deny) always works.
- It names who is asking: an installed plugin, a program in this tab, or a
  **REMOTE** program over ssh. Remote programs get Allow once, **Allow always**
  and Deny (no Never). Allow always is saved under
  `ssh:<user@host:port> <SHA256 host key>/<plugin>`, one entry per capability, so
  another host, a changed host key, or extra capabilities ask again. There is no
  Allow always when the host can't be identified.

Command and MCP plugins run with a scrubbed environment (PATH, HOME, LANG, plus
`MOUSER_PLUGIN_API` and `MOUSER_PLUGIN_NAME`), so tokens and API keys in your
environment don't reach them.

## Command plugins

On the `[mouser]` line (F10), a word that belongs to an enabled command plugin
runs that plugin instead of going to the shell (`!word` still forces the
shell). The first run asks for `run`. Then:

- Mouser sends `{"api":1,"command":"hello","args":["Michael"],"cwd":"..."}` on stdin.
- It reads `{"text":"Hello, Michael!"}`, or `{"error":"..."}`, from stdout (64 KB
  at most).
- The reply becomes a notice. A plugin that runs longer than 10 s is stopped.

Plugins can't take Mouser's own words (`theme`, `font`, `set`, `connect`, …), and
two plugins can't claim the same word.

## MCP plugins

`run.command` starts a stdio MCP server (newline-delimited JSON-RPC 2.0). In
P1, Mouser does `initialize`, `notifications/initialized`, `tools/list` and
`tools/call` (`mouser_core::mcp::Client`); `mouser plugin probe` shows the
result. Since P3 every enabled MCP plugin is also a connection in the sidebar's
MCP section (`plugin:<name>`). It starts once its `run` permission is granted
(click its row), and each tool call asks via the broker with capability
`tool.<name>`. See docs/mcp.md §Client.

## Fuzzball plugins

A program in a tab sends `ESC P = F z p h ; id=1,name=…,caps=a+b ESC \`, and
Mouser answers after the popup (docs/protocol/plugins.md). Grants belong to
that tab and end with it. A `plugin.toml` with `transport = "fuzzball"` is
optional; it only lists the plugin.

## Examples (examples/plugins)

- `hello-command`: `hello [name]` on the `[mouser]` line (POSIX sh).
- `hello-mcp`: a stdio MCP server with a `greet` tool (python3, stdlib only).
- `hello-fuzzball`: `./hello-fuzzball.sh` asks for `notify` + `screen.read` and prints the answer.
- `hello-mode`: a mode on `h` that shows the directory and the time (POSIX sh).

```
mouser plugin install examples/plugins/hello-command
mouser plugin install examples/plugins/hello-mcp && mouser plugin probe hello-mcp
sh examples/plugins/hello-fuzzball/hello-fuzzball.sh   # inside a Mouser tab
```

## How it fits together (P6)

- `mouser_core::plugins::Registry::load()` lists every plugin: built-ins (from
  their compiled-in manifests), installed plugins by priority, then engines in the
  engine order. `modes()` and `engines()` are the enabled ones, best first.
- The app builds the mode switcher with `Host::from_registry` (commander, chat,
  command-transport mode plugins), and the sidebar's engine list and
  `mouser engine list` come from `Registry::engines()`. Engine selection
  (`engines::priority()`, crash sentinels, live switching) is unchanged apart from
  passing over disabled engines.
- The `Mode` trait and the engine C ABI did not change.
