---
title: Demo app: a Flutter client for Mouser's MCP server
status: draft
note: this walkthrough is a draft. Screenshots and clips are still TODOs.
---
# Demo app: a Flutter client for Mouser's MCP server

[mouser_mcp_demo](https://github.com/WeirdBrains/mouser_mcp_demo) is a Flutter
desktop app that talks to [Mouser's MCP server](mcp.html): connect, list the
tools, call any tool from a form generated from its input schema, and follow a
curated Demo page. Its MCP client (`lib/mcp/mcp_client.dart`) is plain Dart
with no extra packages, so it's a good template for your own integration.

> **TODO:** screenshots of the Connect, Protocol, Tools and Demo pages, and a
> short clip of the Demo page.

## What it covers

- **Tools only.** `initialize`, `ping`, `tools/list` and `tools/call`. Mouser
  only advertises `tools`, and answers `-32601` to resources, prompts, logging
  and completion; the Protocol page probes those to show it.
- **Transports.** Streamable HTTP with a session, HTTPS with a CA file or a
  pinned SHA-256, and stdio through `mouser mcp serve --stdio`.
- **All 11 tools:** `tabs_list`, `screen_read`, `scrollback_read`,
  `shell_type`, `shell_run`, `files_list`, `files_read`, `files_mkdir`,
  `files_copy`, `files_move` and `files_trash`.

## 1. Start Mouser's server

```sh
mouser mcp enable        # then restart Mouser; serves http://127.0.0.1:7801/mcp
mouser mcp token --show  # Bearer token for HTTP
```

For a throwaway instance, set `MOUSER_HOME=$(mktemp -d) MOUSER_SECRET_STORE=file`,
then run `mouser mcp enable` and `mouser` in that environment.

## 2. Run the app

```sh
git clone https://github.com/WeirdBrains/mouser_mcp_demo
cd mouser_mcp_demo
flutter run -d linux
```

The repo also has Linux, macOS and Windows packaging scripts; see its README.

## 3. Connect

Pick a transport:

- **Streamable HTTP:** endpoint plus token. The key button runs
  `mouser mcp token --show`; `MOUSER_MCP_URL` and `MOUSER_MCP_TOKEN` prefill them.
- **HTTPS** (`https://127.0.0.1:7802/mcp`): give the CA file
  (`$MOUSER_HOME/mcp/cert.pem`) or pin the SHA-256 from `mouser mcp status`.
- **stdio:** the app spawns `mouser mcp serve --stdio`, which bridges to the
  running Mouser.

The protocol version offered is selectable. Mouser serves 2025-11-25,
2025-06-18 and 2025-03-26, and answers with its newest for anything else. The
Connect page then shows the status and server info.

> **TODO:** screenshot of the Connect page.

## 4. Protocol

Shows the session (server info, capabilities, session id, certificate SHA-256),
`ping` with its round trip, a JSON-RPC batch, the capability probes, a raw
JSON-RPC console, and "End HTTP session" (`DELETE /mcp`).

> **TODO:** screenshot of the Protocol page.

## 5. Tools

Lists every tool from `tools/list`, builds a form from each tool's input schema,
and shows the result of `tools/call`. Write tools (`shell_*`,
`files_mkdir`/`copy`/`move`/`trash`) show Mouser's approval popup first.

> **TODO:** screenshot of the Tools page.

## 6. Demo

A guided run: `tabs_list` then `screen_read`; a `files_list`/`files_read` file
browser; `shell_run` then `screen_read`; plus `scrollback_read`, `shell_type`,
and "send files to Mouser", a local file chooser feeding `files_copy`,
`files_move`, `files_mkdir` and `files_trash`. Mouser takes paths; it has no
upload API.

Tapping a file, or a path chip in any tool result, opens a viewer for text,
images, .docx, PDF, video and audio (Linux needs `mpv` for media).

> **TODO:** clip of the Demo page.

## 7. Check your own client

The repo's smoke test is a compact reference for a full round trip:

```sh
MOUSER_MCP_TOKEN=$(mouser mcp token --show) dart run bin/smoke.dart http
dart run bin/smoke.dart stdio
```
