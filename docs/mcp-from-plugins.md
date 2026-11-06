# Using Mouser's MCP server from a plugin

Mouser is both an MCP **server** (agents and editors drive the terminal) and
an MCP **client** (Mouser calls out to servers you configure, and to MCP
plugins you install). This guide covers the two plugin-facing sides:

1. A **mode plugin** that asks for its own connection to Mouser's server
   (`wants_mcp`).
2. An **MCP plugin** Mouser starts as a stdio server and shows in the sidebar.

The user-facing server and client reference is [Mouser and MCP](./mcp.md).
Mode wire details are in [Mode plugin protocol](./mode-protocol.md); manifests
and permissions are in [Plugins](./plugins.md).

## Mode plugin: `wants_mcp`

In your `Hello`, set `wants_mcp: true`. When Mouser accepts the handshake it
may include an `mcp` grant on `Welcome::Accepted`:

```rust
// crates/mouser-mode-api — shape of the grant
pub struct McpGrant {
    pub endpoint: String, // Unix socket, or \\.\pipe\mouser-mcp-<session> on Windows
    pub token: String,    // scoped; revoked when the plugin stops
    pub client: String,   // "plugin:<name>"; approvals are keyed by it
    pub tools: Vec<String>,
}
```

Connect as an MCP client to that endpoint with the token. Approvals for tool
calls show Mouser's popup under `mcp:plugin:<name>`, the same broker every
other MCP client uses ([MCP §Permissions](./mcp.md#permissions)). When your
process exits, the grant is revoked — do not cache the token across runs.

Typical uses:

- Drive `modes_list` / `modes_switch` / `mode_actions` / `mode_action` against
  **other** modes (your own menu is already available as events; see below).
- Read terminal and tab tools Mouser's server exposes, under the permissions
  the user grants your plugin client.

If the grant is `None`, Mouser's MCP server is off or refused the plugin;
continue without it. Turning the server on is a user action
(`mouser mcp enable`, or Preferences → MCP server).

### Your own mode over MCP (no grant needed)

You do **not** need `wants_mcp` for clients outside Mouser to drive **your**
mode. Answer `menu.open` with the actions that apply; Mouser's
`mode_actions` / `mode_action` tools use that answer
([Mode plugin protocol §Over MCP](./mode-protocol.md)). Checklist:

- Every entry matches the selection and view state.
- Destructive work confirms inside the plugin; Mouser also asks once
  (`modes.control`).
- Answer within 3 s.
- Entry ids containing `delete`, `trash`, `move`, `rename`, `paste` or `cut`
  are flagged `destructive` to the client.

## MCP plugin: Mouser as the client

Ship a stdio MCP server (newline-delimited JSON-RPC 2.0) and a manifest:

```toml
name = "hello-mcp"
version = "0.1.0"
description = "MCP server with a greet tool"
permissions = ["run"]

[[provides]]
kind = "tool-provider"

[run]
transport = "mcp"
command = ["python3", "./server.py"]
```

```sh
mouser plugin install examples/plugins/hello-mcp
mouser plugin permissions hello-mcp run=allow
mouser plugin probe hello-mcp          # initialize + tools/list
```

Mouser runs `initialize`, `notifications/initialized`, `tools/list` and
`tools/call`. Once `run` is granted, the plugin appears in the sidebar's MCP
section as `plugin:<name>`. Each tool call goes through the broker with
capability `tool.<name>`.

`examples/plugins/hello-mcp/server.py` is a stdlib-only reference: one
`greet` tool, no dependencies. Match its framing if you write a server in
another language.

Environment for command and MCP plugins is scrubbed (PATH, HOME, LANG, plus
values you pass with `--env`). Do not rely on the user's interactive shell
profile.

## Security model (both paths)

- **Tokens and keys never appear in plugin-drawn UI.** Mode plugins ask for
  secrets through `prefs.secret_prompt`; MCP clients get a scoped token from
  Mouser, not the user's global MCP token.
- **Popups cannot be spoofed.** Mouser draws them, names the plugin or client,
  and ignores early input ([Plugins §The approval popup](./plugins.md)).
- **Off-loopback MCP** requires HTTPS and the token
  ([Deploying the MCP server](./deploy-mcp.md)).
- A mode plugin with `wants_mcp` still needs its manifest permissions for
  host calls; the MCP grant does not widen file or agent access.

## Which path should I use?

| goal | path |
|---|---|
| Dock UI with views, keys, host calls | mode plugin (`transport = "mode"`) |
| Let Cursor / agents run your mode's actions | answer `menu.open` (server tools) |
| Call Mouser's MCP tools from inside your mode | `wants_mcp: true` and the grant |
| Expose your own tools to Mouser's agent and sidebar | MCP plugin (`transport = "mcp"`) |
| Short-lived grant from a program in the tab | Fuzzball `p h` ([Fuzzball integration](./fuzzball-integration.md)) |

## Checklist

1. Mode + `wants_mcp`: handle a missing grant; drop the client on `stopping`.
2. Mode menus: MCP-safe actions, 3 s budget, destructive confirmations.
3. MCP plugin: `probe` succeeds; sidebar row starts only after `run=allow`.
4. No secret or token is logged, printed in `-v`, or drawn by the plugin.
5. Document the permissions you need in your README and `plugin.toml`.

## See also

- [Mouser and MCP](./mcp.md) — server, client, Let's Encrypt, permissions
- [Deploying the MCP server](./deploy-mcp.md) — bind, HTTPS, ACME
- [Building a mode plugin](./building-modes.md)
- [Plugins](./plugins.md) — transports and the broker
