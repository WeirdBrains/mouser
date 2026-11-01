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
