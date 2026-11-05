---
title: Demo app: a Flutter client for Mouser's MCP server
status: draft
note: this walkthrough is a draft. The demo's repository isn't public yet, so the repo link and screenshots below are TODOs.
---
# Demo app: a Flutter client for Mouser's MCP server

The Mouser MCP demo is a Flutter app that talks to [Mouser's MCP server](mcp.html)
and exercises every tool it offers. It's a worked example of integrating with
the server from your own app.

> **TODO:** link to the public repository once it's published.
>
> **TODO:** screenshots of the Connect, Protocol, Tools and Demo pages.

## What it covers

- **Tools only.** The app uses MCP's `initialize`, `ping`, `tools/list` and
  `tools/call`. It doesn't use resources or prompts.
- **Transports.** HTTP with MCP sessions, HTTPS with either a pinned
  certificate or a CA, and stdio through `mouser mcp serve --stdio`.
- **All 11 tools:** `tabs_list`, `screen_read`, `scrollback_read`,
  `shell_type`, `shell_run`, `files_list`, `files_read`, `files_mkdir`,
  `files_copy`, `files_move` and `files_trash`.

## Walkthrough

The app has four pages.

### 1. Connect

Pick a transport and connect. Turn the server on first, as described in
[Mouser and MCP](mcp.html): HTTP is `http://127.0.0.1:7801/mcp`, HTTPS is
`https://127.0.0.1:7802/mcp`, where you either trust
`$MOUSER_HOME/mcp/cert.pem` or pin the SHA256 that `mouser mcp status` shows.
For stdio, the app starts `mouser mcp serve --stdio`, which bridges to the
running Mouser and reads the token itself. Over HTTP and HTTPS the app needs
the token from `mouser mcp token --show`.

> **TODO:** screenshot of the Connect page.

### 2. Protocol

Shows the MCP exchange itself: the `initialize` handshake, `ping`, and the
session the HTTP transport keeps.

> **TODO:** screenshot of the Protocol page.

### 3. Tools

Lists what `tools/list` returns and lets you call each tool with
`tools/call`. Typing, running commands and changing files go through Mouser's
permission broker, so Mouser shows an approval card for those calls.

> **TODO:** screenshot of the Tools page.

### 4. Demo

A guided run through the tools against a live Mouser.

> **TODO:** screenshot or recording of the Demo page, and a description of
> each step once the app is published.
