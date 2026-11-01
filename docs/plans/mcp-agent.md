# Plan: plugins, MCP and the agent chat mode (t187u-t189u)

Status: **P1-P4b done** (docs/plugins.md, docs/mcp.md) (decisions below are Michael's, t196u-t198u). Written
2026-10-31 after Phase 1 (commander) closed. Each step below has its scope, architecture, dependencies (⚠ = needs Michael's
install approval), security model, tests, rough size and open questions. Section 9
lists the sources behind the facts in it.

## Decisions (Michael, t196u-t198u)

These replace the provisional defaults the first draft proposed.

1. **Plugins connect three ways: MCP, Fuzzball, and a mouser command.** A *command
   plugin* registers commands on the `[mouser]` command line, backed by an external
   executable: Mouser runs it with a JSON request on stdin and reads a JSON reply
   from stdout. (Interpretation; Michael may clarify.) **No native-library plugins in
   v1**: engines keep their own C ABI, nothing else loads code in-process.
2. **Mouser's MCP server (P2) offers stdio, loopback HTTP, and an HTTPS option** (TLS
   with the user's certificate, or a generated self-signed one).
3. **The local model is llama.cpp built in**, plus an option to point at any
   OpenAI- and/or Anthropic-compatible inference URL (llama-server, Ollama, vLLM, …).
4. **The default local model is Gemma 4 E2B**, with a setting to switch to E4B.
5. **Bringing in any model is a plugin.** `model-provider` is a plugin kind; the
   Gemma/llama.cpp plugin is just the first one, and the frontier adapters (OpenAI,
   Anthropic, Google, xAI, Moonshot/Kimi, OpenRouter) are model-provider plugins too.
   Others can ship plugins for other models through the same interface.

## 0. Phases at a glance

| # | Phase | Depends on | Rough size | Ships |
|---|---|---|---|---|
| P1 | Plugin system core: manifest, registry, CLI, permissions, Fuzzball + MCP hooks | – | L (3-4 wk) | `mouser plugin …`, approval prompts, example plugins |
| P2 | Mouser as an MCP server | P1 (permissions) | M (1.5-2 wk) | `mouser mcp serve` (stdio) + loopback Streamable HTTP + HTTPS option, token auth |
| P3 | MCP client + sidebar | P1 | M (1.5 wk) | `[[mcp.servers]]` in preferences.toml, sidebar section |
| P4 | Model providers as plugins | P1 | M-L (2-3 wk) | the `model-provider` plugin kind; frontier adapters as plugins; keys in the OS keychain |
| P4b | Embedded local model plugin (optional) | P4 | M (1.5-2 wk) | the first model plugin: llama.cpp + Gemma 4 E2B (E4B switch), or an OpenAI/Anthropic-compatible URL; downloads only with consent |
| P5 | Agent chat mode | P2, P3, P4 | L (3 wk) | dock above the prompt, tools with per-action approval, transcript, cancel |
| P6 | Convert modes and engines to plugins (t189u) | P1-P5 | M-L (2-3 wk) | commander, chat and the engines loaded through the plugin registry |

Sizes assume one developer at the Phase 1 pace, with tests and docs. P2 and P3 can
run side by side once P1 has landed, and P4 can overlap with them.

## 1. Principles

- **One registry, many hook points.** A plugin is a manifest plus one or more
  *providers* (mode, engine, model provider, tool provider, …). The registry decides
  what loads; each subsystem asks it for its kind.
- **Same lifecycle as engines.** Install, uninstall, priority and skip follow
  `mouser engine …` (docs/engines.md §5): plugins live in
  `$MOUSER_HOME/plugins/<name>/`, there is no install from a URL in v1, and a plugin
  that crashes at start is skipped with a restore command.
- **Nothing acts without consent.** Permissions are declared in the manifest,
  granted by the user on install (or at first use), and stored per plugin. Running or
  typing in a shell, file writes, network access and model downloads always need
  approval, and the agent asks per action.
- **Built-ins are plugins that happen to be compiled in.** Commander, the chat mode
  and the wgpu/skia engines get the same manifests and traits as external plugins,
  so P6 is a registration change, not a rewrite.
- **Local-only by default.** Mouser's MCP server binds 127.0.0.1 (or uses stdio) and
  needs a token. Nothing listens on the network unless the user turns it on.
- **State under `$MOUSER_HOME`.** Tests use a temporary MOUSER_HOME and never touch
  the real ~/.mouser.

## 2. P1: the plugin system

### Scope
- A manifest format and a registry (`mouser_core::plugins`), mirroring
  `mouser_core::engines`.
- CLI: `mouser plugin list | install <dir|plugin.toml> | uninstall <name> |
  enable/disable <name> | priority … | move … | unskip … | permissions <name>`.
- Three transports (decision 1), plus *builtin* for code compiled into Mouser:
  - **MCP**, an out-of-band process over stdio (a local URL comes with P3);
  - **Fuzzball**, in-band escape sequences from a program running *in* the terminal,
    so it works over ssh and tmux;
  - **command**, an executable that adds commands to the `[mouser]` line (JSON on
    stdin, JSON on stdout).
  - No native-library plugins in v1.
- A permission model with approval prompts drawn by Mouser itself (they can't be
  forged by terminal output).
- Plugin types: `mode`, `engine`, `model-provider`, `tool-provider`, plus reserved
  `theme`, `command` and `status` (later).

### Manifest (`plugin.toml`)
```toml
name = "chat"
version = "0.1.0"
api = 1                       # plugin API version
description = "Agent chat mode"
authors = ["WeirdBrains"]

[[provides]]
kind = "mode"                 # mode | engine | model-provider | tool-provider
id = "chat"
hotkey = "a"

[run]                         # how it is reached: mcp | fuzzball | command
transport = "mcp"
command = ["mouser-chat-plugin", "--stdio"]   # mcp over stdio
# transport = "command": command = [...] plus commands = ["hello"] on the [mouser] line

[permissions]                 # asked for on install, shown in `plugin permissions`
screen_read = true            # visible text of the active tab
scrollback = false
shell_type = "ask"            # type into / run in a shell: never | ask | allow
files = { read = ["$CWD"], write = "ask" }
network = ["api.anthropic.com"]
models = "ask"                # may start / download a model
```
- Engines keep `engine.toml`, read as a `plugin.toml` with one `engine` provider, so
  no migration is forced.
- Modes keep their keymap override in `modes.toml`.

### Architecture
- **Registry.** It reads `$MOUSER_HOME/plugins/*/plugin.toml` plus the built-in list,
  keeps a priority order per kind (`plugins.order.mode = [...]` in
  preferences.toml), and keeps skip/failure records (shared code with
  `engines::skips`).
- **Provider traits** (`mouser_render::plugins`):
  - `ModeProvider` creates a `Box<dyn Mode>`, the existing trait;
  - `EngineProvider` is the existing `Engine` factory;
  - `ToolProvider` lists and calls tools (MCP-shaped: name, JSON schema, call → content);
  - `ModelProvider` is defined in P4.
- **Adapters per transport:**
  - *builtin* is a direct Rust impl;
  - *command* runs the executable once per `[mouser]` command: one JSON request
    on stdin, one JSON reply on stdout, with a timeout;
  - *mcp* runs an rmcp client (P3) and maps the plugin's MCP tools and resources
    onto `ToolProvider`; a mode over MCP gets `view`/`action`/`key` as tools that
    return a panel description (JSON of `Panel`, already serde);
  - *fuzzball* adds a new Fuzzball sub-protocol `p` (plugin):
    `DCS = F z p r ; name=…,kind=tool-provider ST` registers a session-scoped plugin.
    Tool calls go back to the program as DCS replies on the PTY. A Fuzzball plugin
    cannot be "installed"; it lives only while its program runs in that tab and is
    always treated as untrusted (all actions ask).
- **Permission broker.** One `Grants` store at `$MOUSER_HOME/plugins/grants.toml`
  (plugin → permission → `allow | ask | never`, with an optional expiry). Every
  provider call that needs a capability goes through `broker.check(plugin, cap,
  detail)`. That returns allow, deny, or ask, and ask shows a Mouser-drawn approval
  popup: what, which plugin, the exact command or path, and Allow once / Always /
  Deny. The popup uses the options-window style and lives outside the terminal grid,
  so terminal output can't fake it.

### Dependencies
- None new for the core: serde and toml are already in. `rmcp` comes in with P3.

### Security
- Install copies only local files. Library plugins run in-process (same trust as
  engines) and are flagged "native code" in `plugin list`.
- MCP plugins run as child processes with a scrubbed environment (no API keys unless
  granted).
- Fuzzball plugins are tab-scoped and untrusted. Over ssh, a remote program can
  *offer* tools but never gets a grant beyond "ask".
- Grants are per plugin name *and* manifest hash; an update that changes the
  permissions asks again.

### Testing
- Unit tests:
  - manifest parse and validation;
  - registry order, skip and failure;
  - grants: allow, ask, never and expiry;
  - the Fuzzball `p` parser with fuzzed bodies;
  - the approval popup layout, hit-tested like the options window.
- Integration: a fake MCP plugin (a tiny Rust test binary over stdio) and a fake
  Fuzzball plugin (a shell script printing DCS), all under a temporary MOUSER_HOME.

### Size and open questions
- Size: **L, 3-4 weeks.**
- Questions for Michael:
  1. ~~C ABI for non-engine plugins?~~ Decided: no native libraries in v1.
  2. Should remote (ssh) Fuzzball plugins be allowed at all by default?
  3. Is `plugins/` next to `engines/` fine, with engines later moving under
     `plugins/`?

## 3. P2: Mouser as an MCP server

### Scope
Expose Mouser to any MCP client (Claude Desktop, Cursor, other agents, and Mouser's
own chat mode).

| Tool / resource | What | Permission |
|---|---|---|
| `tabs.list`, `tabs.focus`, `tabs.new`, `tabs.close` | tabs and titles, cwd/place (local or `user@host:/dir`) | read: allow; others: ask |
| `screen.read` (resource `mouser://tab/<id>/screen`) | visible text with the cursor position | screen_read |
| `scrollback.read` (range, search) | history lines | scrollback |
| `shell.type` | type text into a tab, no Enter | shell_type (ask) |
| `shell.run` | `keep_line` run (the commander path), waits for the prompt (OSC 133 if present), returns the output | shell_type (ask, shows the command) |
| `files.list`, `files.read`, `files.copy`, `files.move`, `files.mkdir`, `files.trash` | commander file ops, local or remote via `fetch` | files.read / files.write (ask) |
| `commander.view` | switch the view, select or open an item | allow |

### Architecture
- `mouser-mcp` crate on `rmcp` (official SDK; 3.8.0 implements spec 2026-07-28 and
  stays compatible with 2025-11-25).
- Transports:
  - **stdio** via `mouser mcp serve --stdio`. Clients start it, and it attaches to
    the running Mouser over a local socket (`$MOUSER_HOME/run/mcp.sock`, 0600; a
    named pipe on Windows).
  - **Streamable HTTP**, in-app, on `127.0.0.1:<port>` only, when enabled.
- The app side is a request queue into the event loop (the same path as UI
  actions), so tools never touch terminal state from another thread.

### Security
- Off by default (`[mcp.server] enabled = false`). Binds loopback only. For HTTP,
  the `Origin` header is validated (the spec requires this against DNS rebinding).
- Auth: a random 256-bit bearer token kept in the OS keychain (`mouser mcp token`
  prints or rotates it). Stdio clients authenticate to the socket with the same token.
- Every mutating tool goes through the P1 broker. The approval names the client
  (from `clientInfo`), not just "MCP".
- The status bar shows "MCP: <client> connected" while a client is attached.
- Rate limits: approvals are queued and never auto-granted while the window is
  unfocused.

### Dependencies
- `rmcp` (Apache-2.0, MSRV 1.88; check the workspace toolchain), tokio (already used?
  confirm), hyper or axum for HTTP (comes with rmcp's transport features), and
  `keyring` for the token.

### Testing
- An rmcp client in tests drives the server over stdio and HTTP under a temporary
  MOUSER_HOME.
- Checks:
  - wrong or missing token gives 401;
  - a bad `Origin` gives 403;
  - mutating calls block until a scripted approval;
  - `shell.run` is checked against a fake PTY.
- Live: Claude Desktop or the MCP Inspector against `mouser mcp serve`, with forced
  focus and screenshots of the approval popup.

### Size and open questions
- Size: **M, 1.5-2 weeks.**
- Questions:
  4. ~~HTTP at all?~~ Decided: stdio, loopback HTTP, and an HTTPS option (user cert
     or self-signed).
  5. Should `shell.run` be allowed in a tab running a full-screen program? (Proposed:
     no, as with the `[mouser]` command line.)
  6. For `files.trash`, use the OS trash, or a Mouser trash under $MOUSER_HOME?

## 4. P3: MCP client and the sidebar

> **Done (as built):**
> - Hand-rolled client on std + rustls, like P2, not rmcp: stdio plus Streamable HTTP(S) with JSON/SSE replies.
> - `[[mcp.servers]]` takes `command`/`env`/`url`/`token`/`ca`/`enabled`; the token is sent as a bearer.
> - Per-server threads reconnect with backoff and ping.
> - The sidebar MCP section has expandable tool lists; the `[mouser]` line has `mcp …`.
> - Broker capability `tool.<name>`.
> - Deferred: resources, prompts, list-changed, `server/discover`, OAuth (Q7), project-local config (Q8), per-server enable toggles in the sidebar.

### Scope
- The user lists MCP servers to use. Mouser connects, lists their tools, resources
  and prompts, and shows them in the sidebar.
- Their tools feed the chat mode (P5) and tool-provider plugins (P1).

### Config (`preferences.toml`)
```toml
[[mcp.servers]]
name = "github"
command = ["github-mcp-server", "stdio"]     # stdio: Mouser starts it
env = { GITHUB_TOKEN = "keychain:github" }   # secrets by reference, never inline
enabled = true

[[mcp.servers]]
name = "docs"
url = "https://example.com/mcp"              # Streamable HTTP
auth = "keychain:docs-token"                 # or "oauth" (rmcp auth feature)
```

### Architecture
- `rmcp` client.
- A per-server task holds the child process or HTTP client. It uses
  `server/discover` on 2026-07-28 servers, falls back to `initialize` on older ones
  (rmcp negotiates this), and subscribes to list changes via `subscriptions/listen`.
- The sidebar gets an "MCP" section listing each server with its state (connected /
  starting / failed + reason) and its tool count. Clicking a server opens a panel of
  its tools (name, description, schema summary). Each server has enable/disable and
  restart.

### Security
- Servers are started only from preferences, never by the agent.
- Env values that reference `keychain:` are resolved at spawn. Plain values are
  allowed but warned about in the sidebar.
- Each tool call goes through the broker with the server name as the "plugin".
  Defaults: read-like tools `ask` once per session, others `ask` each time. MCP tool
  annotations (`readOnlyHint` etc.) only inform the default and are never trusted
  for "allow".

### Testing
- A fake stdio MCP server binary, plus rmcp's HTTP server in a test. Checks:
  - reconnect and failure states;
  - list-changed handling;
  - the sidebar layout via the existing panel tests;
  - secrets never written to preferences.toml (round-trip test).

### Size and open questions
- Size: **M, 1.5 weeks.**
- Questions:
  7. Should OAuth for remote MCP servers be in v1? (rmcp has an auth feature.)
  8. Should a project-local `.mouser/mcp.toml` be supported (like editors do), or
     global only?

## 5. P4: model providers as plugins

> **Done (as built), see docs/models.md:**
> - Hand-rolled streaming HTTP/SSE on std + rustls, not reqwest.
> - Adapters: OpenAI-compatible Chat Completions, Anthropic Messages, and native Google (Gemini's compatible URL goes through openai).
> - `[models]` prefs, `mouser model list/add/remove/test/default`, and keys via `--key-stdin` into the keychain.
> - Built-in model-provider plugins, plus plugins with a `[model]` table (`transport = "http"`).
> - `gemma-local` is stubbed (E2B default, E4B switch).
> - Deferred: Responses API, images, retries, server-side model lists, the `network` grant for plugin hosts, and `mouser model key set` with a no-echo prompt (`add --key-stdin` does that job).

### One interface (`mouser_core::models`)
```rust
trait ModelProvider: Send + Sync {
    fn id(&self) -> &str;                          // "anthropic", "local-gemma", …
    fn models(&self) -> Vec<ModelInfo>;            // name, context, tools?, vision?
    fn chat(&self, req: ChatRequest, sink: &mut dyn ChatSink) -> Result<(), ModelError>;
    // ChatRequest: system, messages (text/image/tool results), tools (JSON schema),
    // tool_choice, max_tokens, temperature (optional; some models fix it)
    // ChatSink: text deltas, tool-call deltas, usage, done; cancel via a token
}
```
- Hosted providers share two wire adapters:
  - **OpenAI-compatible** Chat Completions or Responses: OpenAI, xAI, OpenRouter,
    Moonshot/Kimi (`https://api.moonshot.ai/v1`), and local servers such as
    `llama-server`;
  - **Anthropic Messages**.
  - Google Gemini gets its own adapter, or its OpenAI-compatible endpoint (open
    question).
- **Kimi K2** is about 1T parameters (MoE), so it is hosted-only. Its K2.6 API fixes
  some sampling parameters (no temperature/top_p) and rejects
  `tool_choice="required"`; the adapter has per-model quirks for these.
- Providers are plugins of kind `model-provider` (decision 5): the frontier adapters
  ship as built-in model-provider plugins, and anyone can add a model through an
  external plugin over MCP. The local llama.cpp/Gemma plugin is the first one.

### API keys
- Stored in the OS keychain through the `keyring` crate:
  - macOS Keychain;
  - Windows Credential Manager;
  - Linux Secret Service, with kernel keyutils as the fallback.
- `preferences.toml` holds only `key = "keychain:anthropic"`.
- `mouser model key set anthropic` reads the key from a no-echo prompt and stores it.
- If no keychain is available (a headless Linux box), fall back to
  `$MOUSER_HOME/secrets.toml` (0600) with a warning. Open question: also encrypt it?
- Keys are never logged and never passed to plugins without the `network` grant for
  that provider's host.

### Dependencies
- `keyring` (on Linux it needs D-Bus/Secret Service at runtime, which GNOME Keyring
  or KWallet provide; no build-time system deps), `reqwest` with rustls (no OpenSSL),
  and SSE parsing (`eventsource-stream` or hand-rolled).

### Testing
- A wiremock-style local HTTP fake per adapter, checking streaming, tool-call
  deltas, errors, retries, cancel and the Kimi quirks.
- Keyring tests use the mock store.
- An opt-in live smoke test (ignored by default) with real keys from env.

### Size
**M-L, 2-3 weeks.**

## 6. P4b: the embedded local model (optional plugin)

### Choice of model
**Gemma 4 E2B-it as the default, E4B as the step up.**
- Both have native function calling.
- Licence: Apache-2.0.
- Context: 128K.
- Size: official QAT Q4_0 GGUF, about 2.9 GB for E2B and 4.5 GB for E4B.

FunctionGemma 270M (Gemma 3 based, about 290 MB, Gemma Terms of Use) is the lightest
tool caller. Its own model card and independent benchmarks say it needs fine-tuning:
base multi-turn tool calling scores 10-39%. So it is offered as an "experimental /
tiny" option, not the default. A Mouser-specific FunctionGemma fine-tune is possible
later (open question).

### Runtime
- **`llama-cpp-2`** (Rust bindings to llama.cpp, 0.1.160). It has the broadest GGUF
  architecture coverage, and llama.cpp supports Gemma tool calls and grammar-forced
  JSON.
- `LlamaContext` is `!Send`, so inference runs on a dedicated thread with channels.
- Alternatives considered:
  - **mistral.rs**: a full Rust stack, GGUF and tool calling; but architecture
    support has lagged and one project reported Gemma 4 GGUF and CPU issues.
  - **candle**: too low-level, since we would write the model ourselves.
  - **Talk to `llama-server` or Ollama over the OpenAI-compatible API**: no build
    deps at all. Proposed as the *first* local option, because it reuses the P4
    adapter.

### Consent
- Nothing is downloaded until the user picks a model in the model panel. The panel
  shows size, licence (Apache-2.0 / Gemma Terms) and the source URL (Hugging Face).
  Then:
  - the download goes to `$MOUSER_HOME/models/` with a checksum;
  - it is resumable and cancellable;
  - `mouser model rm <name>` deletes it.

### Dependencies ⚠ (need Michael's approval)
- Building llama.cpp from source: **cmake, clang/libclang (bindgen), a C++ compiler**.
  Arch: `cmake clang`; gcc is already there.
- GPU, optional feature flags:
  - **Vulkan**: `vulkan-headers`, `shaderc`/`glslc` and `spirv-headers` on Arch,
    which suits the RTX 3060 without CUDA;
  - or **CUDA**: the `cuda` toolkit, which is large.
  - macOS uses Metal (no extra install). Windows needs MSVC + CMake in CI.
- CI: the local-model crate is behind a cargo feature (`local-llm`) that is off by
  default, so the three-OS CI does not need these. A separate job builds it on Linux
  (CPU only).
- Package: a separate `mouser-local-llm` plugin package, so the main package keeps
  its dependency list.

### Security
- Models run in-process (library plugin) or as a child `llama-server` (MCP or
  OpenAI-compatible). Their tool calls are suggestions that go through the same
  broker as any provider.

### Testing
- A tiny GGUF (a test model of a few MB) for load, generate and tool-call parse in
  the feature-gated job.
- Download tests against a local HTTP fake (checksum, resume, cancel).

### Size and open questions
- Size: **M, 1.5-2 weeks** (plus CI work).
- Questions:
  9. ~~Which local route first?~~ Decided: llama.cpp built in, plus an
     OpenAI/Anthropic-compatible inference URL option.
  10. ~~E2B or E4B?~~ Decided: E2B default, a setting switches to E4B.
  11. Is a FunctionGemma fine-tune on Mouser's own tools wanted later?

## 7. P5: the agent chat mode

### Scope
- A mode plugin `chat` (hotkey `a`; open question). Like commander it is stateful,
  and only one mode is active at a time, so opening chat swaps commander out.
- The chat docks above the prompt in the same dock slot commander uses, and shows:
  - a transcript of user and assistant messages, tool calls and results, with
    collapsible tool output;
  - an input field;
  - a provider/model picker;
  - Cancel.

### Architecture
- **Context.** By default it is the active tab's visible screen plus the last N
  scrollback lines, the cwd/place and the commander selection, through Mouser's own
  tools (the same ones as the P2 MCP server, called in-process).
- **Tools** come from:
  - Mouser's tools (P2, in-process);
  - tools of the enabled MCP servers (P3);
  - tool-provider plugins (P1).
  All are namespaced (`mouser.shell.run`, `github.create_issue`).
- **Loop.** Send to the `ModelProvider` and stream into the transcript. On a tool
  call, run the broker check, then the approval card *in the dock* (command, args
  and target tab; Allow once / Always for this chat / Deny / Edit), then run the
  tool, add the result and continue.
- **Cancel** (Esc or the button) stops the stream and kills any running tool. Limits
  cap steps per turn and tokens, and show the provider's cost when it reports one.
- **Transcripts** are saved to `$MOUSER_HOME/chats/<date>-<id>.jsonl`; they can be
  turned off or deleted, and are never sent anywhere except the chosen provider.

### Security
- Shell, typing and file-write tools always ask per action. There is no "allow all"
  in v1.
- Screen and scrollback reads stay inside the chosen provider: a local model keeps
  everything on the machine, and a hosted one shows a one-time notice "screen text
  is sent to <provider>".
- Secrets: the screen text is redacted (API-key-like patterns, password prompts)
  before it is sent; this is best effort and the notice says so.
- Prompt injection: tool output and screen text are marked as untrusted in the
  prompt. The approval card always shows the real action, whatever the model says
  about it.

### Testing
- A fake `ModelProvider` with scripted tool calls drives the loop: approval
  allow/deny, cancel mid-stream and mid-tool, the step limit, and transcript save.
- Snapshot tests of the dock layout.
- Live, with forced focus: chat, a run-a-command approval, and a cancel, with
  screenshots on dark and light themes.

### Size and open questions
- Size: **L, about 3 weeks.**
- Questions:
  12. Hotkey and name: "chat" on `a`?
  13. Context default: the screen only, or the screen plus 200 lines of scrollback?
  14. Should transcripts be kept by default?

## 8. P6: modes and engines become plugins (t189u)

### Scope
Register commander, chat and the built-in engines (`mouser`/wgpu, `skia`, `flat`)
through the plugin registry, with their own `plugin.toml`, instead of hard-coded
lists. Behaviour and keymaps stay the same.

### How it stays a refactor
- **Modes:** `Host::builtin()` becomes `registry.modes()`. Commander already
  implements `Mode`; its manifest declares `screen_read=false` and
  `files={read=["*"], write="ask"}`, which matches the pipe action.
- **Engines:** `engine.toml` is read as a `plugin.toml`, and the
  `mouser engine …` CLI becomes an alias for `mouser plugin … --kind engine`. The
  order key `engines = [...]` maps to `plugins.order.engine`. The C engine ABI is
  unchanged.
- **Build:** built-ins stay compiled in (`transport = "builtin"`), so there is no
  dynamic loading of core code. They can be disabled like any plugin, except the
  `mouser` engine and one mode-less fallback.
- **Out-of-tree plugins:** optional, later — commander or skia built as separately
  installable crates/packages once the API is stable.

### Testing
The existing commander and engine tests run unchanged against the registry path. A
test checks that `mouser engine list` and `mouser plugin list --kind engine` agree,
plus a migration test for old preferences.

### Size and open questions
- Size: **M-L, 2-3 weeks.**
- Questions:
  15. Should the `mouser engine` CLI stay as an alias for good?
  16. Should skia move out of the main package once it is a plugin?

## 9. Sources (checked 2026-10-31)

- **MCP specification 2026-07-28 (latest):**
  - transports: stdio and Streamable HTTP; POST-only, with per-request SSE; the
    `MCP-Protocol-Version` header; `Origin` validation and binding to localhost —
    https://modelcontextprotocol.io/specification/2026-07-28/basic/transports and
    https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http
  - changelog: stateless; `server/discover` replaces `initialize`;
    `subscriptions/listen`; multi-round-trip requests; tasks moved to an extension —
    https://modelcontextprotocol.io/specification/2026-07-28/changelog
- **rmcp 3.8.0**, the official Rust SDK (Apache-2.0, MSRV 1.88; implements 2026-07-28
  and is compatible with 2025-11-25) — https://crates.io/crates/rmcp and
  https://github.com/modelcontextprotocol/rust-sdk
- **Go SDK v1.7.0 notes** on the 2026-07-28 wire changes —
  https://github.com/modelcontextprotocol/go-sdk/releases/tag/v1.7.0
- **llama-cpp-2 0.1.160** (builds llama.cpp; needs clang + cmake; `cuda` / `vulkan`
  features) — https://crates.io/crates/llama-cpp-2,
  https://docs.rs/crate/llama-cpp-2/latest/features,
  https://github.com/utilityai/llama-cpp-rs and
  https://github.com/ggml-org/llama.cpp/blob/master/docs/build.md
- **llama.cpp tool-call support** (native formats, lazy grammars) —
  https://github.com/ggml-org/llama.cpp/pull/9639
- **mistral.rs** — https://github.com/EricLBuehler/mistral.rs. A project that moved
  from mistral.rs to llama-cpp-2 (Gemma 4 GGUF and CPU issues):
  https://github.com/SuperSwinkAI/Swink-Agent/pull/596 and
  https://docs.rs/crate/swink-agent/latest/source/specs/022-local-llm-crate/research.md
- **Gemma 4** (E2B/E4B/12B/26B A4B/31B, Apache-2.0, function calling, QAT Q4_0 GGUF) —
  https://huggingface.co/google/gemma-4-E2B-it,
  https://huggingface.co/google/gemma-4-E2B-it-qat-q4_0-gguf,
  https://gemma.ai/models/gemma-4/,
  https://developers.google.com/edge/litert-lm/models/gemma-4 and
  https://ai.google.dev/gemma/docs/get_started
- **FunctionGemma 270M** (Gemma 3 based; Gemma Terms; needs fine-tuning) —
  https://huggingface.co/google/functiongemma-270m-it,
  https://ai.google.dev/gemma/docs/functiongemma,
  https://huggingface.co/unsloth/functiongemma-270m-it-GGUF and
  https://www.distillabs.ai/blog/making-functiongemma-work-multi-turn-tool-calling-at-270m-parameters/
- **Kimi K2** (about 1T-parameter MoE; OpenAI-compatible API; K2.6 parameter quirks) —
  https://github.com/moonshotai/Kimi-K2,
  https://platform.kimi.ai/docs/api/overview and
  https://platform.kimi.ai/docs/api/models-overview
- **keyring** (macOS Keychain, Windows Credential Manager, Linux Secret Service or
  keyutils) — https://docs.rs/keyring/latest/keyring/ and
  https://github.com/open-source-cooperative/keyring-rs
