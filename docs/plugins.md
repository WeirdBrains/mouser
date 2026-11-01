# Plugins

Mouser plugins add modes, tools, model providers and `[mouser]` line commands.
This is phase 1 of docs/plans/mcp-agent.md: the manifest, registry, CLI,
permission broker with approval popups, and the three transports. Mouser's MCP
server (P2), the MCP client in the sidebar (P3), model providers (P4) and the chat
mode (P5) build on it. Built-in modes and engines become plugins in P6.

## Transports

| transport | what it is | lifetime | trust |
|---|---|---|---|
| `mcp` | a program speaking MCP over stdio (`run.command`) | started on demand | installed: asks, then remembers what you choose |
| `fuzzball` | a program running *in* a tab, over Fuzzball `p` (docs/protocol/plugins.md) | while it runs, that tab only | untrusted; over ssh it asks every time |
| `command` | an executable behind `[mouser]` line words: one JSON request on stdin, one JSON reply on stdout | one run per command (10 s limit) | installed: asks, then remembers |

There are no native-library plugins in v1. Engines keep their own ABI
(docs/engines.md) and show up in the registry as kind `engine`.

## The manifest: `plugin.toml`

```toml
name = "hello-command"          # a-z 0-9 - _ ; not a built-in name
version = "0.1.0"
api = 1                         # optional; plugin API version (1)
description = "says hello from the [mouser] line"
permissions = ["run", "notify"] # what it may ask for (below)

[[provides]]                    # one or more
kind = "command"                # mode | model-provider | tool-provider | command
id = "hello"                    # optional; defaults to the name

[run]
transport = "command"           # mcp | fuzzball | command
command = ["sh", "./hello.sh"]  # ./ is the plugin's directory; other names come from PATH
commands = ["hello"]            # command plugins only
```

`kind = "engine"` isn't allowed here: engines install from `engine.toml`, and
`mouser plugin install` hands an engine directory to the engine installer.
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

`mouser engine …` is still there and does the same for engines. Installing from
a URL isn't supported. State lives in `$MOUSER_HOME/plugins/`: one directory per
plugin, `state.toml` (priority, disabled) and `permissions.toml`.

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

```
mouser plugin install examples/plugins/hello-command
mouser plugin install examples/plugins/hello-mcp && mouser plugin probe hello-mcp
sh examples/plugins/hello-fuzzball/hello-fuzzball.sh   # inside a Mouser tab
```

## Designed for P6

- The registry already lists the built-ins: commander as a `mode` and the
  wgpu/skia engines as `engine`. Commander shares the plugins' priority list.
  Built-ins can't be disabled yet; that comes with P6.
- P6 moves their creation behind the registry (`Registry::providers(kind)`)
  without changing the `Mode` trait or the engine ABI.
