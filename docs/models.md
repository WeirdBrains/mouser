# Model providers

P4 of docs/plans/mcp-agent.md. Mouser talks to language models through one
interface, `mouser_core::models::ModelProvider`, which streams text and tool
calls. Every model is a plugin of kind `model-provider` (decision 5):

| Provider | Kind of plugin | Wire API | Default URL |
|---|---|---|---|
| `openai` | built in | OpenAI Chat Completions | `https://api.openai.com/v1` |
| `xai` | built in | OpenAI-compatible | `https://api.x.ai/v1` |
| `openrouter` | built in | OpenAI-compatible | `https://openrouter.ai/api/v1` |
| `moonshot` (Kimi) | built in | OpenAI-compatible | `https://api.moonshot.ai/v1` |
| `anthropic` | built in | Anthropic Messages | `https://api.anthropic.com` |
| `google` | built in | Gemini `streamGenerateContent` | `https://generativelanguage.googleapis.com` |
| `gemma-local` | built in, **stub** | llama.cpp (P4b) | — |
| any name | `[[models.providers]]` | openai / anthropic / google | yours |
| any name | installed plugin with `[model]` | openai / anthropic / google | the plugin's |

## Commands

```sh
mouser model list                                  # providers, models, * = default
printf %s "$KEY" | mouser model add anthropic --key-stdin   # a built-in: key only
mouser model add openai --key-stdin --model gpt-5 --model gpt-5-mini   # first = its default

# Any compatible URL (llama-server, Ollama, vLLM, LM Studio, a proxy, …)
mouser model add local --api openai --url http://127.0.0.1:8080/v1 --model qwen3
mouser model add proxy --api anthropic --url https://llm.example.com --key-stdin --model claude-x
# Gemini's OpenAI-compatible endpoint instead of the native one:
mouser model add gemini-compat --api openai --url https://generativelanguage.googleapis.com/v1beta/openai --key-stdin --model gemini-2.5-flash

mouser model default anthropic/claude-sonnet-4-5   # or just a provider: its first model
mouser model test                                  # stream one reply from the default
mouser model test local/qwen3 --prompt "hi" --tool # --tool offers get_time and requires a call
mouser model remove local                          # also deletes its key
```

## Configuration

```toml
[models]
default = "anthropic/claude-sonnet-4-5"

[[models.providers]]
name = "anthropic"
api = "anthropic"                 # openai | anthropic | google
url = "https://api.anthropic.com"
key = "keychain:model-anthropic"  # a reference; the key is in the OS keychain
models = ["claude-sonnet-4-5"]
# ca = "/path/ca.pem"             # trust a self-signed https server
```

- Keys go to the OS keychain (Keychain, Credential Manager, Secret Service) via
  `--key-stdin`; with no keychain, a 0600 file in `$MOUSER_HOME/secrets/` with a
  warning. They are never printed, logged or written to preferences.toml, and are
  sent only to that provider's URL. Error messages show the server's message, never
  the request.
- Plain `http://` is accepted only for this computer (local servers); remote
  providers need `https://` (Mozilla roots, or `ca`).
- Auth: OpenAI-compatible `Authorization: Bearer`, Anthropic `x-api-key` +
  `anthropic-version: 2023-06-01`, Google `x-goog-api-key`. No key, no header (local).

## The interface

```rust
trait ModelProvider: Send + Sync {
    fn id(&self) -> &str;
    fn models(&self) -> Vec<ModelInfo>;
    fn chat(&self, req: &ChatRequest, sink: &mut dyn ChatSink) -> Result<()>;
}
// ChatRequest { model, system, messages, tools: Vec<ToolSpec{name, description, schema}>,
//               tool_choice: Auto|None|Required|Tool(name), max_tokens, temperature }
// Message { role: User|Assistant, parts: Text | ToolCall{id,name,args} | ToolResult{id,name,content,is_error} }
// ChatSink::event(Text | ToolStart{index,id,name} | ToolArgs{index,delta} | Usage{input,output} | Done{stop})
// ChatSink::cancelled() is polled between stream events; true stops the request.
```

`stop` is normalised to `end`, `tool_use`, `max_tokens` (or the provider's own
word). `models::Collect` assembles a reply into an assistant `Message`.

Per-model quirks: Kimi K2 models (`kimi-k2*`) get no `temperature`, and
`tool_choice = Required` is sent as `auto` (Moonshot rejects `required`). Anthropic
requires `max_tokens` (default 4096).

## Model-provider plugins

A plugin can bring a model by declaring an HTTP endpoint (see
`examples/plugins/ollama-model`):

```toml
name = "ollama-model"
version = "0.1.0"
[[provides]]
kind = "model-provider"
[run]
transport = "http"
[model]
api = "openai"
url = "http://127.0.0.1:11434/v1"
default = "llama3.2"
models = ["llama3.2", "qwen3"]
```

## The local model (P4b, not built yet)

`gemma-local` is declared (crates/mouser-core/src/models/gemma-local.toml) with
**Gemma 4 E2B as the default** and **E4B** as the switch
(`mouser model default gemma-local/gemma-4-e4b`). Building llama.cpp and
downloading the weights wait for Michael's OK; until then using it explains that.

## Not yet

Responses API, images, retries/backoff, listing models from the server
(`/models`), the `network` grant for plugin-declared hosts, and the chat mode that
uses all this (P5).
