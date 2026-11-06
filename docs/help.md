# Help

The guides in `docs/` are built into Mouser, so the same text is on the web and
in the terminal:

```sh
mouser help                    # every topic and its title
mouser help models             # one guide (ids are file names: models, mcp, windows, …)
mouser help search trackpad    # topics that mention all the words, title matches first
mouser help model add          # a command's usage, when no guide has that name
```

Topic ids are the file names under `docs/` without `.md` (`protocol` for
`docs/protocol/README.md`, `index` for `docs/README.md`). Archived plans under
`docs/historical/` and internal maintainer docs (`deploy-site`, `release`,
`HANDOVER`, mapped `internal` in `sections.toml`) are not topics. Guides link
each other with relative links, which name the topic to open next.
