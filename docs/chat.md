# Chat mode (the agent)

`chat` is a mode plugin, like commander. `esc :a Enter` (or `:a` on the F10 line) swaps
commander out, and the chat docks above the prompt in the same slot. `esc :c` brings
commander back. The conversation stays while you switch, and a running turn keeps
running.

It answers questions about your terminal and can act on it through tools. Every
action that types, runs or writes shows an **approval card** with the real command,
path and tab first.

## Keys

All keys can be remapped in `modes.toml` under `[chat.keys]`; `ctrl+shift+k` lists them.

| Key | Does |
|---|---|
| `alt+i` (or click the input) | type a message; Enter sends, Ctrl+U clears |
| `esc`, `alt+x`, Ctrl+C, the **cancel** button | cancel the running turn (the stream stops, an open card is denied, a running tool is abandoned) |
| `alt+y` / `alt+a` / `alt+n` | card: allow once / allow for this chat / deny |
| `alt+l` | new chat |
| `alt+pageup` / `alt+pagedown`, wheel | scroll the transcript |
| click a tool row | show or hide its output |

Cards are answered only with these alt keys or a click, so ordinary typing can never
approve anything. With nothing running, `esc` leaves the input and keys go back to
the shell.

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

## Model

The chat uses `mouser model default` at each turn (docs/models.md). `gemma-local` works
in builds with `--features local-llm`, or through `mouser model local url`. Any
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
- `mouser_render::modes::chat::ChatMode`: the dock (header, transcript, card, input).
  Tests are in `crates/mouser-render/tests/chat_render_tests.rs`.
- `crates/mouser-app/src/chat_host.rs`: the app's host (tabs, AppTools with
  `preapproved`, MCP clients, broker checks).
- `cargo run --release -p mouser-app --features local-llm --example chat_live -- "PROMPT"`:
  a headless live run against the default model with a sandboxed `sh` tool.

## Not yet

- The plan's **Edit** button on the card.
- A model picker in the dock (use `mouser model default`).
- Showing cost.
- Images.
- More than one chat at a time.
- Killing a running MCP call on cancel: the call is abandoned and its result ignored.
