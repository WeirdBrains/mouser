# ratmark compatibility report

37 documents (cases plus Mouser's README and docs), widths [24, 40, 80].

## Totals

| renderer | rows | rows equal to Mouser's | docs with Mouser's words | over width (grid) | over width (unicode) | grapheme splits | rows with control chars |
|---|---|---|---|---|---|---|---|
| mouser | 23857 | 23857 | 111/111 | 0 | 1 | 1 | 0 |
| ratmark 6483e72 | 28021 | 3659 | 26/111 | 3 | 0 | 0 | 5 |
| ratmark fix | 28021 | 3659 | 26/111 | 3 | 0 | 0 | 5 |

## Per case (width 24)

| case | renderer | rows | grid over | unicode over | splits | control |
|---|---|---|---|---|---|---|
| case:headings | mouser | 7 | 0 | 0 | 0 | 0 |
| case:headings | ratmark 6483e72 | 7 | 0 | 0 | 0 | 0 |
| case:headings | ratmark fix | 7 | 0 | 0 | 0 | 0 |
| case:wrap | mouser | 5 | 0 | 0 | 0 | 0 |
| case:wrap | ratmark 6483e72 | 5 | 0 | 0 | 0 | 0 |
| case:wrap | ratmark fix | 5 | 0 | 0 | 0 | 0 |
| case:lists | mouser | 9 | 0 | 0 | 0 | 0 |
| case:lists | ratmark 6483e72 | 9 | 0 | 0 | 0 | 0 |
| case:lists | ratmark fix | 9 | 0 | 0 | 0 | 0 |
| case:code-quote | mouser | 6 | 0 | 0 | 0 | 0 |
| case:code-quote | ratmark 6483e72 | 6 | 0 | 0 | 0 | 0 |
| case:code-quote | ratmark fix | 6 | 0 | 0 | 0 | 0 |
| case:links | mouser | 6 | 0 | 0 | 0 | 0 |
| case:links | ratmark 6483e72 | 6 | 0 | 0 | 0 | 0 |
| case:links | ratmark fix | 6 | 0 | 0 | 0 | 0 |
| case:table | mouser | 4 | 0 | 0 | 0 | 0 |
| case:table | ratmark 6483e72 | 5 | 0 | 0 | 0 | 0 |
| case:table | ratmark fix | 5 | 0 | 0 | 0 | 0 |
| case:wide-table | mouser | 3 | 0 | 0 | 0 | 0 |
| case:wide-table | ratmark 6483e72 | 6 | 0 | 0 | 0 | 0 |
| case:wide-table | ratmark fix | 6 | 0 | 0 | 0 | 0 |
| case:streaming | mouser | 6 | 0 | 0 | 0 | 0 |
| case:streaming | ratmark 6483e72 | 6 | 0 | 0 | 0 | 0 |
| case:streaming | ratmark fix | 6 | 0 | 0 | 0 | 0 |
| case:graphemes | mouser | 3 | 0 | 0 | 1 | 0 |
| case:graphemes | ratmark 6483e72 | 2 | 1 | 0 | 0 | 0 |
| case:graphemes | ratmark fix | 2 | 1 | 0 | 0 | 0 |
| case:graphemes-code | mouser | 2 | 0 | 0 | 0 | 0 |
| case:graphemes-code | ratmark 6483e72 | 1 | 1 | 0 | 0 | 0 |
| case:graphemes-code | ratmark fix | 1 | 1 | 0 | 0 | 0 |
| case:cjk | mouser | 3 | 0 | 0 | 0 | 0 |
| case:cjk | ratmark 6483e72 | 3 | 0 | 0 | 0 | 0 |
| case:cjk | ratmark fix | 3 | 0 | 0 | 0 | 0 |
| case:control | mouser | 2 | 0 | 0 | 0 | 0 |
| case:control | ratmark 6483e72 | 2 | 0 | 0 | 0 | 2 |
| case:control | ratmark fix | 2 | 0 | 0 | 0 | 2 |

## Streaming (width 40, at most 400 prefixes per document)

`back`: rows of one prefix's layout that changed or went away in the next, counted from the end. 1 is the row being written.

| renderer | steps | max back | steps with back > 1 | steps with back > 3 | worst at |
|---|---|---|---|---|---|
| mouser | 10170 | 9 | 315 | 43 | docs/cli.md @2589 `…OUSER_OPEN_SIDEBAR' \| Op` |
| ratmark 6483e72 | 10170 | 37 | 641 | 201 | docs/engines.md @30805 `…\| single process (18f728` |
| ratmark fix | 10170 | 37 | 641 | 201 | docs/engines.md @30805 `…\| single process (18f728` |

## Worst streaming documents (ratmark fix, width 40)

| ratmark max back | mouser max back | document, where |
|---|---|---|
| 37 | 3 | docs/engines.md @30805 `…\| single process (18f728` |
| 36 | 9 | docs/cli.md @2589 `…OUSER_OPEN_SIDEBAR' \| Op` |
| 35 | 5 | README.md @8481 `… mouser engine list     ` |
| 32 | 5 | docs/mode-protocol.md @2420 `…efs.write' \| written to ` |
| 23 | 7 | docs/HANDOVER.md @18437 `… audio sample clock (min` |
| 22 | 6 | docs/mcp.md @2019 `… **yes** ('shell.type') ` |
| 22 | 5 | docs/modes.md @11745 `…s) \| same \| same \|⏎\| q (` |
| 21 | 8 | docs/plugins.md @1334 `…shes in 30 s \| installed` |

## First content difference per document (ratmark fix vs Mouser, width 80)

| document | mouser | ratmark |
|---|---|---|
| case:lists | one two nested 1. first 2. | one two ◦ nested 1. first |
| case:control | evil �]52;c;AAAA� text and a | evil ␛]52;c;AAAA␇ text and a |
| README.md | emulator (Linux-first) <p align="center"><img src="resources/icons/png/mouser_256.png" alt="Mouser | emulator (Linux-first) Mouser is a new |
| docs/HANDOVER.md | if you don't have it). Features | if you don’t have it). Features |
| docs/README.md | Gemma MCP: Mouser's MCP server (HTTPS, | Gemma MCP: Mouser’s MCP server (HTTPS, |
| docs/chat.md | with esc :a , a click, | with esc :a, a click, or |
| docs/cli.md | login shell; eve -- CMD Same | login shell; everything after it is |
| docs/connection-manager.md | drag the sidebar's left edge (the | drag the sidebar’s left edge (the |
| docs/deploy-mcp.md | MCP server Mouser's MCP server (MCP) | MCP server Mouser’s MCP server (MCP) |
| docs/deploy-site.md | page; the crate's own spec site | page; the crate’s own spec site |
| docs/downloads.md | mouser plugin instal mouser engine instal | mouser plugin install mouser engine install |
| docs/engines.md | from the image x, y, drawn | from the core’s font stack (8-bit |
| docs/help.md | models, mcp, win dows, ) mouser | models, mcp, w mouser help search |
| docs/kitty-compliance.md | CSI > fl ✅ Stack per | CSI > ✅ Stack per screen |
| docs/markdown.md | and the model's replies (Chat). Links | and the model’s replies (Chat). Links |
| docs/mcp.md | (P3, Client). Mouser's MCP server Mouser | (P3, Client). Mouser’s MCP server Mouser |
| docs/mode-protocol.md | goes to Mouser's plugin log. The | goes to Mouser’s plugin log. The |
| docs/models.md | OpenAI Chat Comple https://api.opena grok built | OpenAI Chat https://api.openai Completions .com/v1 grok |
| docs/modes.md | reset to config.toml's size (saved to | reset to config.toml’s size (saved to |
| docs/plugins.md | three transports. Mouser's MCP server (P2), | three transports. Mouser’s MCP server (P2), |
| docs/preferences.md | keys Mouser doesn't know are kept, | keys Mouser doesn’t know are kept, |
| docs/release.md | and Windows: cargo fmt --check cargo | and Windows: ◦ cargo fmt --check |
| docs/rendering.md | dingbats like oh-my-zsh's ➜), then from | dingbats like oh-my-zsh’s ➜), then from |
| docs/storage.md | the download manager's jobs while Mouser | the download manager’s jobs while Mouser |
| docs/terminal.md | and scrollback; dragg Double-click / triple-click | and scrollback; dragging past the top/bottom |
| docs/themes.md | Star (default) Dark: foreground #E6FBFA, background | Star (default) ◦ Dark: foreground #E6FBFA, |
| docs/windows.md | calls the terminal's ITerminalHandoff3::EstablishPtyHandoff COM method | calls the terminal’s ITerminalHandoff3::EstablishPtyHandoff COM method |

## Samples (width 24)

### case:graphemes

mouser:
```
👨‍👩‍👧🇯🇵❤️éé👨‍👩‍👧👨‍👩‍
👧🇯🇵🇯🇵 and 👨‍👩‍👧
family éééé
```
ratmark 6483e72:
```
👨‍👩‍👧🇯🇵❤️éé👨‍👩‍👧👨‍👩‍👧🇯🇵🇯🇵 and 👨‍👩‍👧
family éééé
```
ratmark fix:
```
👨‍👩‍👧🇯🇵❤️éé👨‍👩‍👧👨‍👩‍👧🇯🇵🇯🇵 and 👨‍👩‍👧
family éééé
```
### case:wide-table

mouser:
```
a      │ b      │ c
───────┼────────┼───────
aaaaa… │ bbbbb… │ ccccc…
```
ratmark 6483e72:
```
a      │ b      │ c
───────┼────────┼───────
aaaaaa │ bbbbbb │ cccccc
aaaaaa │ bbbbbb │ cccccc
aaaaaa │ bbbbbb │ cccccc
aa     │ bb     │ cc
```
ratmark fix:
```
a      │ b      │ c
───────┼────────┼───────
aaaaaa │ bbbbbb │ cccccc
aaaaaa │ bbbbbb │ cccccc
aaaaaa │ bbbbbb │ cccccc
aa     │ bb     │ cc
```
### case:code-quote

mouser:
```
fn main() {}
    indented

│ quote
│
│ │ deeper
```
ratmark 6483e72:
```
 fn main() {}
     indented

▎ quote
▎
▎ ▎ deeper
```
ratmark fix:
```
 fn main() {}
     indented

▎ quote
▎
▎ ▎ deeper
```
### case:control

mouser:
```
evil �]52;c;AAAA� text
and a [link](http://a�b)
```
ratmark 6483e72:
```
evil ␛]52;c;AAAA␇ text
and a [link](http://a␛b)
```
ratmark fix:
```
evil ␛]52;c;AAAA␇ text
and a [link](http://a␛b)
```
