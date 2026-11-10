# Chat mode (the agent)

`chat` is a mode plugin, like commander. `esc :a Enter` (or `:a` on the F10 line) swaps
commander out, and the chat docks above the prompt in the same slot. `esc :c` brings
commander back. The conversation stays while you switch, and a running turn keeps
running.

It answers questions about your terminal and can act on it through tools, like
Grok Bot: it can run commands in a terminal on this computer and read their output.
Every computer action (typing, running, writing) first shows an **approval card**
with the real command, folder, path or tab, and waits for you: **Allow**, **Allow
all** or **Deny**. Nothing runs without that answer.

## The chat window

The layout follows grokbot-tui, with rounded corners:

- the header: the model (click it, or `m`, to switch), **⌕ models** (the Hugging
  Face browser), **new**, **⤢** (fullscreen, below), and while it applies
  **allow all: on ✕** (click to turn Allow all off) and **cancel esc**;
- a rounded frame titled `Chat: MODEL · STATE` (ready, thinking…, waiting for
  you, allow all) around the conversation: your messages in tinted bubbles on the
  right, the model's replies in bubbles on the left (Markdown), tool calls as
  `⚙` rows between them (a terminal command reads `$ COMMAND`);
- the input line at the bottom: `> ` and what you type.

**Fullscreen**: **⤢** in the header (or the host's `F`, or its ⤢ at the bottom of
the panel) makes the chat fill the terminal space it has; **⤡** or `F` again gives
the shell its rows back. The choice is kept like the panel's height
(`[window] dock_full`).

## Commands in the input

| Command | Does |
|---|---|
| `/hf [words]` | open the Hugging Face browser, searching for the words if given |
| `/model` | switch model (as `m`) |
| `/new` | new chat (as `l`) |
| `/revoke` | turn Allow all off |
| `/help` | list these |

Tab completes the commands, file paths (a word starting `/`, `.` or `~` that is
not a command) and earlier messages. Start a message with `//` to send a leading
`/`. ↑ on an empty input brings back the last message.

## Allow, Allow all, Deny

Each card is Mouser's approval popup, never drawn by the chat, and answered only
with a click or its keys (`y`, `a`, `n`/Esc), after a short arming delay:

- **Allow** (`y`): this one call.
- **Allow all** (`a`): this call and every later computer action in this chat
  without asking, until **new** (a new chat), `/revoke`, the header's
  **allow all: on ✕**, or Mouser quits. It is never saved. A capability you set to
  *never* (`mouser plugin permissions chat shell.type=never`) still stays blocked.
- **Deny** (`n`, Esc): the call does not run; the model is told "the user denied
  this call" and is asked not to retry it.

The default is always to ask.

## Running commands

`mouser.terminal_run` runs a command line on this computer in your shell
(`$SHELL -c`, `cmd /C` on Windows), with no terminal and stdin closed, and returns
its output and exit code to the model:

- it runs in the active tab's folder (a remote tab: your home folder, since it
  runs here, not on the remote host), or the absolute `cwd` the model gives;
- the card shows the exact command and the folder;
- it stops after 120 s (the model may ask for up to 900 s) or when you cancel
  the turn, and what it started stops with it, also when it ends on its own
  (it is not a job runner): on Linux and macOS it runs as its own process
  group, on Windows in a Job Object that ends every process in it; output is
  capped at 64 KiB per stream;
- this is cleanup, not a sandbox: the command runs as you, with your files and
  network. A program that leaves the group on purpose keeps running: on Unix
  one that starts a new session (`setsid`, a daemon that detaches), on Windows
  one started with breakaway from the job where that is allowed, or one
  started in the first instant before the shell is put in the job;
- `TERM=dumb`, `PAGER=cat` and `GIT_PAGER=cat` keep programs from waiting for a
  pager.

To type into a tab's own shell instead (so you see it there), the model uses
`mouser.shell_run`, which asks the same way.

## Models from Hugging Face

**⌕ models** in the header, `h`, `/hf`, or "⌕ Search Hugging Face…" at the top of
the model picker opens the browser in the chat's frame:

1. Type part of a name (`qwen 0.5b`, `gemma`) and press Enter: the GGUF models
   that match, most downloaded first, with downloads (↓) and likes (♥). Tab
   completes earlier searches and the ids shown.
2. ↑/↓ and Enter, or a click, opens a model: its card summary, downloads, likes,
   license, task, architecture, parameters, context length, last update, and its
   GGUF files by quantization (Q4_K_M, Q8_0, F16, …) with sizes, ✓ when installed.
   It also says what Mouser can tell before downloading: "supported
   architecture, not load-tested" when the bundled llama.cpp knows its
   architecture (the file itself is only tried on the first message), or
   "won't run" and why, with no Install, for an architecture it doesn't know or a
   model that is not a chat model (embeddings, speech).
3. Choose a quantization (the smallest Q4 is preselected) and click **Install**
   (or Enter). Mouser's popup asks first with the repository, file, size, license
   and folder. The download shows a progress bar here (and in the chat's status
   line once the browser is closed); **cancel** stops it, and Install again
   resumes it. Each file is checked against the SHA256 Hugging Face publishes.
4. When it is done it is the current model (`gemma-local/NAME`), so the next
   message uses it. An installed one shows **Use** instead of Install.

Esc goes back from a model to the list, then closes the browser (it keeps the
search). More in [Models](./models.md#hugging-face) (`mouser model hf` does the
same from the command line).

## Pictures, video and sound

A picture, video or sound named in a message or in a tool's output (a Markdown
image or link such as `![plot](/tmp/plot.png)`, or a local path like
`/tmp/out.wav` or `~/Pictures/cat.jpg`) gets a player under it:

- **pictures** (png, jpeg, gif, webp, bmp): a thumbnail, never scaled up;
- **video** (mp4, mkv, webm, mov, avi, m4v, ogv): ▶ plays it in place, with the
  time and a progress bar;
- **sound** (mp3, wav, flac, ogg, opus, m4a, aac): ▶ / ❚❚, the time and progress.

Click a picture or **⤢** to open it fullscreen within the chat's space: it flies
from the thumbnail with the same ease-in-out and dimmed backdrop as commander's
viewer (off with reduced motion). There, Space (or a click on a video) plays and
pauses, ←/→ seek 10 s, and Esc, `f`, Enter or **⤡ close** fly it back. Video and
sound use Mouser's own player (ffmpeg and ffprobe on PATH; sound through your
`[audio]` output, docs/preferences.md) and stop when you leave the chat.

Reading files needs the `files.read` permission, asked the first time: until then
each one shows "▶ show NAME here (allow reading files)". Web URLs are not
fetched; only files on this computer are shown.

## Keys

Keys are plain and work while chat has the keyboard (switching to it with `esc :a`, a click, or ctrl+alt+k; Esc or ctrl+alt+k gives it back, docs/modes.md). All keys can be remapped in `modes.toml` under `[chat.keys]`; `ctrl+shift+k` lists them.

| Key | Does |
|---|---|
| `i` (or click the input) | type a message; Enter sends, Ctrl+U clears |
| `esc`, `x`, Ctrl+C, the **cancel** button | cancel the running turn (the stream stops, an open card is denied, a running tool is told to stop: MCP servers get `notifications/cancelled`, and a stdio server that hasn't stopped within 1 s is killed and reconnected) |
| `y` / `a` / `n` | card: Allow / Allow all (this chat) / Deny |
| `m` (or click the model name in the header) | switch model: "Search Hugging Face…", then the configured models; Esc keeps the current one. The choice becomes the default (as `mouser model default`) and applies from the next message |
| `h` | search Hugging Face for a model to install |
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
- elsewhere: switch model (`m`), search Hugging Face models (`h`), turn Allow
  all off, new chat (`l`), scroll up / down (PageUp / PageDown), type, cancel.

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
| The terminal (chat only) | `mouser.terminal_run` | asks on every call (`shell.type`), unless Allow all is on |
| Mouser's own (the P2 MCP server's, in-process) | `mouser.screen_read`, `mouser.shell_run`, `mouser.files_trash`, … | reads run unless the broker says never for `chat`; `shell_*` and `files_mkdir/copy/move/trash` **ask on every call** unless Allow all is on |
| Connected MCP servers and MCP plugins (P3) | `SERVER.TOOL`, `plugin:NAME.TOOL` | the server's broker entry (`mcp-server:NAME` / plugin, `tool.NAME`); *ask* shows the card (Allow all covers these too) |

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
- `plugins/chat` (`mouser-chat`): the dock (header, frame, bubbles, input), a mode
  plugin; `src/hub.rs` the Hugging Face browser, `src/media.rs` the players;
  `plugins/chat/tests/through_mouser.rs` drives it through Mouser.
- `crates/mouser-media`: the player (ffmpeg + cpal), pictures for a view
  (`still`), media named in text (`detect`) and the fullscreen ease (`anim`),
  shared with commander's viewer.
- `crates/mouser-core/src/exec.rs`: `terminal_run`'s runner (timeout, cancel,
  output caps); `crates/mouser-core/src/models/hf.rs`: Hugging Face search,
  details and install.
- `crates/mouser-app/src/chat_host.rs`: the app's host (tabs, AppTools with
  `preapproved`, the terminal tool, MCP clients, broker checks).
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
with `agent.set_default`; `agent.revoke_all` turns Allow all off (it is turned on
only in Mouser's popup). The Hugging Face browser uses `models.hf_search`,
`models.hf_details`, `models.hf_install` (Mouser asks, downloads and sends
`models.download` progress) and `models.hf_cancel`. Media files are read only
with `files.read`.

Keys: `i` type, `Enter` send, `x` cancel, `m` model, `h` Hugging Face, `l` clear,
`PageUp`/`PageDown` scroll. With no model the picker offers the Hugging Face
search; hosted providers are set up in the preferences mode or with
`mouser model add`. Tested through Mouser's StdioMode with a fake provider in
`plugins/chat/tests/through_mouser.rs`.

## Not yet

- The plan's **Edit** button on the card.
- Showing cost.
- Sending pictures to a model (vision); models' mmproj files are not installed.
- Gated Hugging Face models (they need a Hugging Face login).
- More than one chat at a time.
