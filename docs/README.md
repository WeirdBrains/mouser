# Mouser Docs

One guide per feature. The same guides are built into Mouser: `mouser help TOPIC`
([Help](./help.md)). Finished plans are archived under `historical/`.

## Using Mouser

- [Command line](./cli.md): flags, commands, environment, man pages
- [Help](./help.md): the built-in guides
- [Terminal emulation](./terminal.md): input, scrolling and trackpads, sequences, `mouser show`
- [Modes](./modes.md): input modes, commander (list, grid, carousel), remote files
- [Chat](./chat.md): the agent chat mode
- [Markdown rendering](./markdown.md): commander previews, chat replies, `mouser md`
- [Connection manager](./connection-manager.md): SSH hosts
- [SSH and TERM](./ssh.md): `TERM=mouser` where the host knows it, `xterm-256color` where it does not
- [Themes](./themes.md)
- [Preferences](./preferences.md)
- [Storage](./storage.md): `~/.mouser` and `MOUSER_HOME`

## Rendering

- [Rendering and fonts](./rendering.md)
- [Rendering engines](./engines.md): GPU engines, priority order, live switching, crash protection, the session server / UI split
- [Kitty keyboard compliance](./kitty-compliance.md)
- [Fuzzball protocol](./protocol/README.md)

## Extending Mouser

- [Plugins](./plugins.md): modes, engines, model and tool providers, commands
- [Downloads](./downloads.md): installing, updating and rolling back plugins and engines; signatures and checks
- [Mode plugin protocol](./mode-protocol.md): how mode plugins talk to Mouser (stdio, MessagePack, shared-memory images, restarts)
- [Building a mode plugin](./building-modes.md): first mode on `mouser-mode-api`, install and checklist
- [Building a custom rendering engine](./custom-engines.md): C ABI engine, the `flat` example, crash fallback
- [Integrating an app with Fuzzball](./fuzzball-integration.md): graphics, keyboard, in-tab plugins
- [Using Mouser's MCP server from a plugin](./mcp-from-plugins.md): `wants_mcp` grants and MCP plugins
- [Models](./models.md): providers, grok (`xai-`, `grok_`, `--desktop`), the picker, local Gemma
- [MCP](./mcp.md): Mouser's MCP server (HTTPS, Let's Encrypt) and MCP clients

## Platforms

- [Windows](./windows.md): default terminal handoff

## Deploying

- [Deploying the MCP server](./deploy-mcp.md)

## Maintainer notes (repo only)

These stay in the repository and are **not** published on mouser.weirdbrains.com
(`internal` in [`sections.toml`](./sections.toml)):

- [Deploying the website](./deploy-site.md)
- [Release process](./release.md)
- [Handover notes](./HANDOVER.md)
