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

Restart Mouser after changing it. The settings live in `preferences.toml`.

If another Mouser window already holds `port` / `https_port`, the new window
binds an ephemeral loopback port instead and writes a log line — it does not
show an error toast. Clients that need a fixed URL should talk to the first
window, or use `mouser mcp serve --stdio`.

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
| `files_mkdir`, `files_copy`, `file_move` | create, copy, move or rename (never overwrite). `files_move` is `file_move` | **yes** (`files.write`) |
| `files_trash` | move to `$MOUSER_HOME/trash` (nothing is deleted for good) | **yes** (`files.write`) |
| `file_sync` | sync two local paths (`push`, `pull` or `both`) | **yes** (`files.write`) |
| `remote_file_sync` | sync a local path with a path on the tab's host, or `host` | **yes** (`files.write`) |
| `file_download`, `file_upload` | remote → local, and local → remote (directories recursive) | **yes** (`files.write`) |
| `list_themes`, `get_theme` | built-in and user themes; one theme's dark and light values | no (`themes.read`) |
| `set_theme` | make a theme active (saved, and the open window picks it up) | **yes** (`themes.write`) |
| `create_theme`, `duplicate_theme`, `edit_theme` | create, copy, or merge values into a user theme | **yes** (`themes.write`) |
| `delete_theme` | delete any theme except the last one (user files go to the trash; a built-in is hidden) | **yes** (`themes.write`) |
| `modes_list` | the modes: name, hotkey, which is on, whether it has the keyboard | no (`screen.read`) |
| `modes_switch` | turn a mode on by name or hotkey (its own again: modes off) | **yes** (`modes.control`) |
| `mode_actions` | every action of the current mode for its selected item and view state (its context menu) | no (`screen.read`) |
| `mode_action` | run one of those actions by `id` and `arg`, as its key would | **yes** (`modes.control`); file changes ask again |

Relative paths are resolved in the tab's local folder. A tool argument `tab`
picks a tab; the default is the active one.

## Sync, download and upload

`file_sync` copies between two local paths. `direction` is `push` (`from` →
`to`), `pull` (`to` → `from`) or `both` (the default: a file that exists on
only one side is copied to the other). `remote_file_sync` is the same with a
local path and a remote path: `push` uploads, `pull` downloads, `both` is the
default. The remote host is the tab's ssh session, or `host` (and optional
`port`) when you pass one. `file_download` is that pull (`remote` → `to`).
`file_upload` is that push (`from` → `remote`). There is no URL download.

`conflict` is `newer` (the default) or `refuse`. `newer` copies a file only
when its mtime is strictly later than the other side; the same mtime is left
alone. `refuse` copies nothing that already exists with a different size or
mtime, and reports it as a conflict. A file that would be replaced is moved
to `$MOUSER_HOME/trash` first. A replaced remote file is not deleted on the
host: its previous bytes are written into that trash, then the path is
replaced. `dry_run` lists the copies, trashes and skips and writes nothing.
Directories need `recursive` (default true). Dotfiles are included; symlinks
are skipped. The result counts `copied`, `trashed`, `skipped`, `unchanged`
and `bytes`.

`file_move` never overwrites. `files_move` calls the same code.

## Themes

`list_themes` and `get_theme` are reads. `get_theme` returns the `dark` and
`light` variants: foreground, background, cursor, accent, palette and
selection colours.

`set_theme` stores the name in `preferences.toml` and drops this window's
cached colours, so the next frame uses it. A colour preview open in the
appearance panel is cleared; a font preview is not.

`create_theme` starts from Red Star. Any colour you omit stays Red Star.
`variant` is `dark`, `light` or `both` (default `both`): `both` writes the
colours you gave into each variant. A `palette` shorter than 16 colours is
padded with the default ANSI palette. A value that is not `#rgb` or `#rrggbb`
is an error and nothing is written.

`duplicate_theme` copies both variants. `name` is the copy's name; without
it the name is `<theme> copy`, then `<theme> copy 2`, and so on.

`edit_theme` merges the colour fields you send into a user theme. Omitted
fields stay. `variant` picks which file (`both` is the default). A built-in
theme is read-only; the error tells you to `duplicate_theme` first.

`delete_theme` removes a theme from the list. The last one is refused
(`'{name}' is the last theme; Mouser keeps at least one`). A user theme's
directory is moved to `$MOUSER_HOME/trash`, not unlinked. A built-in cannot
be removed from the program: its name is appended to
`$MOUSER_HOME/themes/.hidden` (one name per line) and `list_themes` skips
it. Any copy of that built-in already on disk is trashed too, and a later
start does not write it back. Delete that line to show the built-in again.
The name stays reserved while it is hidden.

If the deleted theme is the one in preferences (or the default, when no theme
is saved), Mouser saves another: the default theme when that is still listed,
otherwise the first theme that remains. This window drops its colour cache so
the next frame uses it.

## Mode actions

The modes' actions are the same registry as their keys and right-click menus
(docs/modes.md §Context menus), so whatever a mode offers you, it offers an MCP
client, and nothing else:

1. `modes_list` shows the modes and which one is on; `modes_switch` turns one on
   (`{"mode": "commander"}` or its hotkey).
2. `mode_actions` asks the current mode for its context menu for the **selected
   item** and the **view state**, exactly what `.` would show: a folder gives
   open and sync, a file copy, move, rename, delete and copy path, a fullscreen
   image close, zoom and scroll, a clip play and fullscreen. Each entry:

   ```json
   {"id": "menu:delete", "arg": 3, "label": "delete…", "key": "", "enabled": true, "destructive": true}
   ```

   Commander's `arg` is the item's index in the listing (for `menu:sort`, the
   sort order).

   `key` is the action's key as bound in `modes.toml` (empty: menu only);
   `enabled: false` entries are listed but refused. A plugin has 3 s to answer;
   a mode with no menu reports that it has no actions.
3. `mode_action` runs one (`{"id": "menu:open", "arg": 3}`). Only ids from the
   mode's key bindings or its current `mode_actions` are accepted. It asks you
   on Mouser's approval popup (`modes.control`, key `mcp:<client>`), naming the
   mode and the action. A `destructive` action (delete, move, rename, paste)
   then asks again the way the mode always does, on its own approval card, and
   never overwrites; the result has `"confirm": true` to tell the client a
   second question is on screen. The effect is applied as if you pressed the
   key: a sync types nothing, an open opens, a copy path fills the clipboard.

The menu Mouser would draw is not shown for an MCP request. Moving the
selection is itself an action (the arrow keys' ids, `down`, `up` ...), so a
client can walk the list, then ask again for the new item's actions.

## Security

- Nothing listens unless you enable it. It binds **127.0.0.1** unless you set
  `bind` for HTTPS (below). Mouser refuses to listen on any other address
  without HTTPS and a token, and plain HTTP always stays on 127.0.0.1.
- **A token is required** on HTTP and HTTPS. It is 32 random bytes, compared in
  constant time. It is kept in the OS keychain: macOS Keychain, Windows
  Credential Manager, or Secret Service on Linux. Without a keychain it goes to
  `$MOUSER_HOME/secrets/mcp-token` (0600), and Mouser warns you.
  `MOUSER_SECRET_STORE=file` forces the file.
- The `Host` and `Origin` headers must be loopback, or one of the names in
  `[mcp.server.tls] domains` (an `Origin` must then be `https://`), which
  blocks DNS-rebinding attacks from web pages. Bodies are capped at 4 MiB.
- Typing, running, file changes and theme changes go through the plugin permission broker
  (docs/plugins.md) under `mcp:<client name>`. They show Mouser's approval
  popup with the exact text, command or path, and the call waits up to 5 minutes
  for your answer. Choose Always or Never there, or later with
  `mouser plugin permissions mcp:<client> …`. A `never` for `screen.read`,
  `scrollback.read`, `files.read` or `themes.read` also blocks those reads.
- The client name comes from the client itself (`clientInfo.name`). The token
  is what proves access; the name only keeps grants apart.

## Let's Encrypt

To reach the server from other machines with a certificate every client
trusts, let Mouser get one from Let's Encrypt and renew it itself.

Before you start:

1. A **public DNS name** (A/AAAA record) that points at this computer, e.g.
   `mcp.example.com`. Let's Encrypt doesn't issue for IP addresses,
   `localhost` or private names.
2. The CA must reach you from the internet. For the default `tls-alpn-01`
   challenge that's **port 443** of that name, forwarded to Mouser's
   `https_port` (or set `https_port = 443` if Mouser may bind it). For
   `http-01` it's **port 80** to `http_port`. Open them in your router (port
   forwarding) and firewall, e.g. `sudo ufw allow 443/tcp`, or allow Mouser in
   the Windows Defender Firewall / macOS firewall prompt. Ports under 1024
   need privileges on Linux (`sudo setcap cap_net_bind_service=+ep $(which mouser-app)`)
   or a forward from 443 to 7802.
3. `dns-01` needs neither port: Mouser runs your `dns_hook` (or asks you, in
   `mouser mcp cert renew`) to add a `_acme-challenge` TXT record. It is the
   only way to get a wildcard (`*.example.com`). There are no DNS provider
   integrations; the hook is any command, e.g. one calling your provider's CLI.
   It gets `MOUSER_ACME_ACTION` (`present`/`cleanup`), `MOUSER_ACME_DOMAIN`,
   `MOUSER_ACME_NAME` and `MOUSER_ACME_TXT`.

```toml
[mcp.server]
enabled = true
https = true
https_port = 7802
bind = "0.0.0.0"                  # listen on the network (HTTPS + token only)

[mcp.server.tls]
source = "acme"                   # self-signed | files | acme
domains = "mcp.example.com"       # comma-separated
email = "you@example.com"         # optional: Let's Encrypt expiry notices
# staging = true                  # Let's Encrypt's staging CA, for trying it out
# challenge = "http-01"           # tls-alpn-01 (default) | http-01 | dns-01
# http_port = 80                  # http-01 listener (answers only the challenge)
# dns_hook = "/usr/local/bin/my-dns-hook"
# directory = "https://…/directory"  # another ACME CA
```

All of these are in preferences mode under *MCP server* (dropdowns for the
source and challenge); saving keeps your comments.

- Mouser asks for the certificate when it starts and serves HTTPS with it as
  soon as it arrives. It renews **30 days before expiry** in the background
  and swaps the new certificate in without a restart; open connections keep
  the old one.
- A failed attempt keeps the current certificate, is logged, and is retried
  after 1 h, 2 h, 4 h, … up to once a day. That stays within Let's Encrypt's
  [rate limits](https://letsencrypt.org/docs/rate-limits/). A forced renewal
  is refused within a day of the last certificate.
- The account key, certificate, private key and renewal state live in
  `$MOUSER_HOME/mcp/acme/`, readable only by you (0600).
- Production is the default; `staging = true` gives untrusted test
  certificates with looser limits. Changing the names or the CA gets a new
  certificate.

```sh
mouser mcp cert status          # source, names, expiry, next renewal, last error
mouser mcp cert renew           # renew now if due (asks the running Mouser)
mouser mcp cert renew --force   # even if not due
```

Clients then use `https://mcp.example.com/mcp` (or `:7802` without a forward)
with the token, and no CA file. Anyone who can reach the port can try the
token, so keep it secret, and rotate it (`mouser mcp token --rotate`) if it
leaks; approvals for typing, running and file changes still pop up on your
screen.

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
