# Mouser and MCP

Mouser is both an MCP **server** (P2, below) and an MCP **client** (P3,
[Client](#client-mouser-uses-mcp-servers)).

# Mouser's MCP server

Mouser can act as an MCP server, so agents and editors can see and drive your
terminal with your approval: its tabs, screens, scrollback, shell and files.
This is P2 of docs/plans/mcp-agent.md. The server is **off by default**.

## Turn it on

```
mouser mcp enable            # loopback HTTP on 127.0.0.1:7801
mouser mcp enable --https    # plus HTTPS on 127.0.0.1:7802 (self-signed, or your cert)
mouser mcp status            # endpoints, certificate fingerprint, where the token lives
mouser mcp disable
```

Restart Mouser after changing it. The settings live in `preferences.toml`:

```toml
[mcp.server]
enabled = true
port = 7801
https = true
https_port = 7802
# cert = "/path/fullchain.pem"   # both or neither; unset = self-signed in $MOUSER_HOME/mcp/
# key  = "/path/key.pem"
```

## Transports

| transport | for | how |
|---|---|---|
| stdio | clients that start a server process | `mouser mcp serve --stdio`, which bridges to the running Mouser (reads the token itself) |
| HTTP | local clients that connect to a URL | `http://127.0.0.1:7801/mcp`, Streamable HTTP, JSON replies |
| HTTPS | clients that require TLS | `https://127.0.0.1:7802/mcp`; trust `$MOUSER_HOME/mcp/cert.pem` or pin the SHA256 shown by `mouser mcp status` |

A stdio client config:

```json
{"mcpServers": {"mouser": {"command": "mouser", "args": ["mcp", "serve", "--stdio"]}}}
```

HTTP and HTTPS clients send `Authorization: Bearer <token>`. Get the token
with `mouser mcp token --show`, and replace it with `--rotate`.

## Tools

| tool | what it does | asks you? |
|---|---|---|
| `tabs_list` | tabs: index, title, active, local folder or remote host, at prompt | no |
| `screen_read` | the visible text of a tab | no |
| `scrollback_read` | the last N lines (≤ 5000) of a tab's scrollback | no |
| `shell_type` | type text into a tab (no Enter) | **yes** (`shell.type`) |
| `shell_run` | run one command line, only when the shell is at its prompt | **yes** (`shell.type`) |
| `files_list`, `files_read` | list a folder, read up to 1 MiB of a file | no |
| `files_mkdir`, `files_copy`, `files_move` | create, copy, move or rename (never overwrite) | **yes** (`files.write`) |
| `files_trash` | move to `$MOUSER_HOME/trash` (nothing is deleted for good) | **yes** (`files.write`) |

Relative paths are resolved in the tab's local folder. A tool argument `tab`
picks a tab; the default is the active one.

## Security

- Nothing listens unless you enable it. It binds **127.0.0.1 only**.
- **A token is required** on HTTP and HTTPS. It is 32 random bytes, compared in
  constant time. It is kept in the OS keychain: macOS Keychain, Windows
  Credential Manager, or Secret Service on Linux. Without a keychain it goes to
  `$MOUSER_HOME/secrets/mcp-token` (0600), and Mouser warns you.
  `MOUSER_SECRET_STORE=file` forces the file.
- The `Host` and `Origin` headers must be loopback, which blocks DNS-rebinding
  attacks from web pages. Bodies are capped at 4 MiB.
- Typing, running and file changes go through the plugin permission broker
  (docs/plugins.md) under `mcp:<client name>`. They show Mouser's approval
  popup with the exact text, command or path, and the call waits up to 5 minutes
  for your answer. Choose Always or Never there, or later with
  `mouser plugin permissions mcp:<client> …`. A `never` for `screen.read`,
  `scrollback.read` or `files.read` also blocks those reads.
- The client name comes from the client itself (`clientInfo.name`). The token
  is what proves access; the name only keeps grants apart.

# Client: Mouser uses MCP servers

P3 of docs/plans/mcp-agent.md. Mouser connects to the MCP servers you list and
to every enabled MCP plugin (`transport = "mcp"`, docs/plugins.md), shows their
tools in the sidebar, and calls them only through the permission broker. The
chat mode (docs/chat.md) uses the same connections.

## Add a server

```sh
# A stdio server: Mouser starts it (scrubbed environment + --env values).
mouser mcp servers add hello -- python3 /path/to/server.py
mouser mcp servers add gh --env GITHUB_TOKEN=keychain:gh -- github-mcp-server stdio

# A Streamable HTTP(S) server. The token is read from stdin into the OS keychain.
printf %s "$TOKEN" | mouser mcp servers add docs --url https://example.com/mcp --token-stdin

# Mouser's own server (P2), e.g. from a second profile; trust its self-signed cert.
mouser mcp token --show | mouser mcp servers add self --url https://localhost:7802/mcp \
    --token-stdin --ca ~/.mouser/mcp/cert.pem

mouser mcp servers list            # configured servers and MCP plugins
mouser mcp servers test docs       # connect and list its tools
mouser mcp servers test docs --call search --args '{"q":"mouser"}'   # asks first
mouser mcp servers remove docs     # also deletes its token and permissions
```

They are stored in `preferences.toml`; secrets are only references:

```toml
[[mcp.servers]]
name = "docs"
url = "https://example.com/mcp"
token = "keychain:mcp-server-docs"   # sent as Authorization: Bearer
# ca = "/path/ca.pem"                # trust this cert/CA instead of the public roots

[[mcp.servers]]
name = "gh"
command = ["github-mcp-server", "stdio"]
env = { GITHUB_TOKEN = "keychain:gh" }   # plain values work but are warned about
# enabled = false                        # keep it listed but don't connect
```

- `keychain:NAME` is the OS keychain (Keychain, Credential Manager, Secret
  Service), or the 0600 file fallback in `$MOUSER_HOME/secrets/` with a warning.
- Plain `http://` is only accepted for this computer (127.0.0.1, localhost, ::1).
  Remote servers need `https://`; certificates are checked against the Mozilla
  roots unless `ca` is set.
- Replies may be JSON or SSE; the `Mcp-Session-Id` is kept for the connection.
- Mouser offers protocol 2025-06-18 and accepts what the server answers.

## Connections

Each server has a background connection. It reconnects with backoff (1 s, doubling
to 5 min) after a failure and pings every 30 s; a stdio server that exits is
restarted. Changes to the list apply on the next start.

An MCP plugin also needs its `run` permission before Mouser starts it. Until then
its row says **ask**; click it to see the approval popup (Allow once starts it for
this session, Allow always remembers).

## Sidebar

The **MCP** section (below the connections, above Engines) lists every server
with its state: connecting, connected, failed (with the reason and the retry
time), ask, denied, off.

- Click a connected server (`▸ name (N)`) to show or hide its tools.
- Click a failed or connecting server to retry now.
- Click a tool to call it with no arguments. A tool with required arguments shows
  what it needs instead; use the `[mouser]` line for those.

`[mouser]` line (F10): `mcp` (states), `mcp tools ID`, `mcp reconnect ID`,
`mcp call ID TOOL [JSON]`, e.g. `mcp call hello greet {"name":"Michael"}`. The
result is shown as a notice.

## Permissions

Every call goes through the broker. The "plugin" is `mcp-server:<name>` for a
configured server and the plugin's own name for an MCP plugin; the capability is
`tool.<tool name>`. The first call shows the approval popup (Allow once, Allow
always, Deny, Never). Tool annotations such as `readOnlyHint` are never trusted
to allow anything.

```sh
mouser plugin permissions mcp-server:docs                    # what is saved
mouser plugin permissions mcp-server:docs tool.search=allow  # or ask / never
mouser plugin permissions 'mcp-server:docs' revoke
```

`mouser mcp servers test --call` asks the same question in the terminal (y once,
a always) and refuses without a terminal.

## Not yet

Resources, prompts, list-changed notifications, OAuth and a project-local
`.mouser/mcp.toml` are not in P3 (plan questions 7 and 8).
