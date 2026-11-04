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
- [Models](./models.md): providers, grok (`xai-`, `grok_`, `--desktop`), the picker, local Gemma
- [MCP](./mcp.md): Mouser's MCP server (HTTPS, Let's Encrypt) and MCP clients

## Platforms

- [Windows](./windows.md): default terminal handoff

## Deploying and releasing

- [Deploying the MCP server](./deploy-mcp.md)
- [Deploying the website](./deploy-site.md)
- [Release process](./release.md)
