# Chat mode (the agent)

`chat` is a mode plugin, like commander. `esc :a Enter` (or `:a` on the F10 line) swaps
commander out, and the chat docks above the prompt in the same slot. `esc :c` brings
commander back. The conversation stays while you switch, and a running turn keeps
running.

It answers questions about your terminal and can act on it through tools. Every
action that types, runs or writes shows an **approval card** with the real command,
path and tab first.

## Keys

Keys are plain and work while chat has the keyboard (switching to it with `esc :a`, a click, or ctrl+alt+k; Esc or ctrl+alt+k gives it back, docs/modes.md). All keys can be remapped in `modes.toml` under `[chat.keys]`; `ctrl+shift+k` lists them.

| Key | Does |
|---|---|
| `i` (or click the input) | type a message; Enter sends, Ctrl+U clears |
| `esc`, `x`, Ctrl+C, the **cancel** button | cancel the running turn (the stream stops, an open card is denied, a running tool is told to stop: MCP servers get `notifications/cancelled`, and a stdio server that hasn't stopped within 1 s is killed and reconnected) |
| `y` / `a` / `n` | card: allow once / allow for this chat / deny |
| `m` (or click the model name in the header) | switch model: configured models, then local/hosted ones to install; ● marks the current one, Esc keeps it. The choice becomes the default (as `mouser model default`) and applies from the next message; not while a turn runs |
| `l` | new chat |
| PgUp / PgDn, wheel | scroll the transcript |
| click a tool row | show or hide its output |
| click a link in a message | open it (web, mail and file links only) |

Your messages and the model's replies are drawn as Markdown: headings, bold and
italic, inline code, code blocks on a background (syntax highlighted by their
language tag), lists, quotes, links and tables
([Markdown rendering](./markdown.md)). A reply that is still streaming in is laid
out as far as it has come; the rows above stay where they are.

Cards are answered only with these alt keys or a click, so ordinary typing can never
approve anything. With nothing running, `esc` leaves the input and keys go back to
the shell.

## Context menu

Right-click (or `.` while chat has the keyboard; docs/modes.md §Context menus):

- on a message (yours, an answer, a note or an error): copy, quote in a reply
  (the text as `> ` lines in the input), retry (your message again; on an
  answer, the message it answered), delete from view, type a message (`i`),
  cancel the answer (`x`);
- on a tool call card: show / hide the output, copy the output, copy the call,
  delete from view, type, cancel;
- elsewhere: switch model (`m`), new chat (`l`), scroll up / down
  (PageUp / PageDown), type, cancel.

"Delete from view" only hides the row; the agent's history is unchanged (a new
chat clears it). Approval cards are still answered only in Mouser's popup.

## What it sees

Each message carries the active tab's place (folder or remote host), its visible
screen and the last 200 scrollback lines (`CONTEXT_LINES`).

- **Secrets are redacted, best effort.** API-key-like tokens (`sk-`, `ghp_`, `xoxb-`,
  `AKIA…` and so on), `password=`/`token:` values, `Authorization: Bearer …`, text after a
  password prompt and private key blocks are replaced with `[redacted]`. The same
  filter runs on tool output.
- Screen text and tool output are wrapped in `<untrusted source=…>` and the system
  prompt says to treat them as data, never instructions. The card always shows the
  real action, whatever the model claims.
- A hosted provider shows a one-time note per chat: "Screen text is sent to
  PROVIDER". A local model (gemma-local, or any loopback URL) keeps everything on
  the machine.

## Tools and permissions

| Tools | Names | Permission |
|---|---|---|
| Mouser's own (the P2 MCP server's, in-process) | `mouser.screen_read`, `mouser.shell_run`, `mouser.files_trash`, … | reads run unless the broker says never for `chat`; `shell_*` and `files_mkdir/copy/move/trash` **ask on every call** (no "allow for this chat") |
| Connected MCP servers and MCP plugins (P3) | `SERVER.TOOL`, `plugin:NAME.TOOL` | the server's broker entry (`mcp-server:NAME` / plugin, `tool.NAME`); *ask* shows the card, and "allow for this chat" is remembered until a new chat |

Tool-provider plugins are MCP plugins, so they appear in the second row once
enabled. Names sent to the model replace dots with `__` (`mouser__shell_run`).
`mouser plugin permissions chat shell.type=never` blocks a capability for the chat.

## Picking a model

If there is no usable default model (none set, a hosted preset without a key, or the
local model not downloaded), `esc :a` opens a picker in the dock instead of the chat. A
message typed before that is kept and sent once a model is ready. A chat restored at
launch (the last mode) does not open the picker until you use it.

The picker lists: Gemma 4 E2B local (default, ~2.84 GB), Gemma 4 E4B local
(~4.59 GB), OpenAI, xAI, OpenRouter, Kimi (Moonshot), Anthropic, Google, and a custom
OpenAI- or Anthropic-compatible URL. ↑/↓ and Enter or a click choose; Esc steps back.

- **Local**: a consent screen shows the size, the source and the folder
  (`MOUSER_MODELS_DIR`, else `[models.local] dir`, else `$MOUSER_HOME/models`). Nothing
  downloads until you say so. A progress bar follows; Esc cancels (downloading again
  resumes); the file is checked against its pinned SHA256.
- **Hosted**: the API key field shows only dots. Tab completes the model name or URL
  from known values when it can; otherwise it moves to the next field (from the key
  field, that is the optional model name, and for a custom URL the URL and API).
  Enter sends one short test request with the
  key in memory; only if it works is the key stored in the OS keychain (or a 0600 file,
  with a warning) and the provider saved.
- Then it becomes the default (`[models] default`) and the chat continues.

## Model

The chat uses `mouser model default` at each turn (docs/models.md). `gemma-local` is built
in (`mouser model local enable`), or runs through `mouser model local url`. Any
configured provider works too. The header shows `provider/model`.

## Limits and transcripts

- At most 8 tool steps per turn (`MAX_STEPS`). After that the model must answer, and
  saying "continue" goes on. Replies are capped at 1024 tokens; tool output at
  8,000 characters.
- Transcripts are kept in `$MOUSER_HOME/chats/<date>-<id>.jsonl` (0600) and rewritten
  after each turn. Turn this off with `[chat] transcripts = false` in preferences.toml.
  Delete the files to remove them; they never go anywhere except the chosen provider.

## Code

- `mouser_core::agent`: `Chat` (send, cancel, answer, clear), `AgentHost` (context,
  tools, check, call, model), redaction and untrusted marking, transcripts. Tested
  with a scripted fake provider in `crates/mouser-core/tests/agent_tests.rs`.
- `plugins/chat` (`mouser-chat`): the dock (header, transcript, card, input), a mode
  plugin; `plugins/chat/tests/through_mouser.rs` drives it through Mouser.
- `crates/mouser-app/src/chat_host.rs`: the app's host (tabs, AppTools with
  `preapproved`, MCP clients, broker checks).
- `cargo run --release -p mouser-app --features local-llm --example chat_live -- "PROMPT"`:
  a headless live run against the default model with a sandboxed `sh` tool.

## Layout

The input line and status sit above the host's dock chrome (the ⤢ fullscreen
button and the focus hint at the bottom), so they are never covered. See
`dock_chrome_height` in `mouser-mode-api`.

## As a plugin program

`plugins/chat` (`mouser-chat`) is the chat mode as a separate program, built only on
`mouser-mode-api` with its `markdown` feature. Installed as a `transport = "mode"`
plugin named `chat`, it is the chat mode (hotkey `a`; Mouser has no built-in chat
since 1.4.0). It needs the
`agent.use` permission. Opening chat the first time asks for that grant (via the
approval popup) when the plugin calls `agent.models` / `agent.state` — it does not
hard-fail with "is not allowed". The agent itself still runs inside Mouser: the plugin sends
`agent.send` / `agent.cancel` / `agent.clear` and draws the transcript from
`agent.state` events, so your keys and models never leave Mouser. Tool-call cards are
drawn by Mouser's approval popup, not the plugin; the plugin only shows "waiting for
your answer". The header's model button lists `agent.models` and sets the default
with `agent.set_default`.

Keys: `i` type, `Enter` send, `x` cancel, `m` model, `l` clear, `PageUp`/`PageDown`
scroll. There is no model setup wizard in the plugin yet; with no model it points to
the preferences mode or `mouser model add`. Tested through Mouser's StdioMode with a
fake provider in `plugins/chat/tests/through_mouser.rs`.

## Not yet

- The plan's **Edit** button on the card.
- A model picker in the dock (use `mouser model default`).
- Showing cost.
- Images.
- More than one chat at a time.
