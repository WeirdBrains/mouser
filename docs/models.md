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
| `gemma-local` | built in (feature `local-llm`) | llama.cpp, or a compatible URL | — |
| any name | `[[models.providers]]` | openai / anthropic / google | yours |
| any name | installed plugin with `[model]` | openai / anthropic / google | the plugin's |

## First run

With no usable model, the chat (`esc :a`) shows a picker that sets one up: local Gemma
(consent, download, SHA256) or a hosted provider (key into the keychain, connection
test). See docs/chat.md §Picking a model. Code: `mouser_core::models::setup`.

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

## The local model (`gemma-local`)

`gemma-local` is Mouser's own model: **Gemma 4 E2B** by default and **E4B** as the
switch (`mouser model default gemma-local/gemma-4-e4b`). It runs on llama.cpp built
into Mouser (Vulkan on Linux/Windows, Metal on macOS) or, instead, on any
OpenAI/Anthropic-compatible server you name.

| Weights | File (ggml-org on Hugging Face) | Size |
|---|---|---|
| `gemma-4-e2b` (default) | `gemma-4-E2B-it-Q4_0.gguf` | 2.84 GB |
| `gemma-4-e4b` | `gemma-4-E4B-it-Q4_0.gguf` | 4.59 GB |

```sh
mouser model local                 # engine, server, models dir, what is downloaded
mouser model local download        # shows the size and asks; --yes skips the question
mouser model local download gemma-4-e4b
mouser model local delete gemma-4-e2b
mouser model local dir /mnt/big/models   # or MOUSER_MODELS_DIR=…; --clear for the default
mouser model local url http://127.0.0.1:8080/v1 [--api anthropic] [--model NAME] [--key-stdin]
mouser model local url --clear     # back to the built-in engine
mouser model test gemma-local --tool     # a tool call, a demo result and the reply
```

In the app, the `[mouser]` line `model download [gemma-4-e4b]` opens an approval popup
with the size, source and destination. It offers only **Allow once** or **Deny**,
asks every time and is never saved as a permission. The download then runs in the
background with progress notices.

- **Nothing downloads without consent.** `local::download` needs a `Consent` that only the prompt
  or popup creates. The `.part` file resumes; size and SHA256 (pinned per file) are checked,
  and a mismatch deletes it.
- **Models dir:** `$MOUSER_MODELS_DIR`, else `[models.local] dir`, else `$MOUSER_HOME/models`.
- **`[models.local]`:** `dir`, `url`, `api`, `key` (a `keychain:` ref), `model`,
  `gpu_layers` (default all) and `context` (default 8192).
- **Prompting:** the GGUF's own chat template (Jinja, rendered with minijinja), with a
  Gemma-format fallback. Tools are described in the system turn, and the model answers
  with `<tool_call>{"name":…,"arguments":…}</tool_call>`, which becomes the usual
  ToolStart/ToolArgs events.
- **Timings:** set `MOUSER_LLM_TIMINGS=1` to print load/prompt/generation speed to stderr.

### Building it

The default build has no llama.cpp. Build the feature with:

```sh
cargo build --release -p mouser-app --features local-llm
# Arch: MOUSER_LOCAL_LLM=1 makepkg   (packaging/arch/mouser-git)
```

It needs cmake and clang, plus Vulkan headers, shaderc and spirv-headers on Linux
(Arch: `cmake clang vulkan-headers shaderc spirv-headers`), and a Vulkan driver at run
time (`vulkan-icd-loader` + the GPU's ICD). `crates/mouser-llm` is excluded from the
workspace, so `cargo build/test --workspace` and CI never compile llama.cpp. Without the
feature, `gemma-local` explains this and still works through `mouser model local url`.

Measured on an RTX 3060 (Vulkan, E2B Q4_0, warm cache): about 0.9 s to load, about
2,800-5,500 tokens/s on the prompt and about 90-110 tokens/s generating. The NVIDIA
driver compiles shaders on first use, so the very first long prompt can take
several seconds.

## Not yet

Responses API, images, retries/backoff, listing models from the server
(`/models`), and the `network` grant for plugin-declared hosts. The chat mode (docs/chat.md)
uses the default model.
