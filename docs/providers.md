# Model providers

Silent Nexus is model-agnostic. Providers implement one trait
(`nexus-models::ModelProvider`) and are selected per task class with fallback.

## Supported providers

| `provider` | Server | Transport |
|---|---|---|
| `llamacpp` | llama.cpp `llama-server` | OpenAI-compatible `/v1/chat/completions` |
| `openai` | OpenAI (GPT) | OpenAI API; base_url defaults to `https://api.openai.com/v1`, API key required |
| `codex` | OpenAI Responses / ChatGPT Codex backend | Isolated Codex OAuth or API-key profile |
| `claude-plan` | Official Claude CLI subscription | Consent-gated, stateless `stream-json` print bridge with all tools disabled |
| `anthropic` | Anthropic Messages API | Native Messages/tool-use/SSE transport with `x-api-key` |
| `openai_compatible` | vLLM, LM Studio, text-generation-webui, … | OpenAI-compatible |
| `custom_http` | Any OpenAI-shaped endpoint | OpenAI-compatible |
| `ollama` | Ollama | Native `/api/chat` (NDJSON) |
| `mock` | (tests) | in-process scripted responses |

The `/connect` catalog also includes first-class OpenAI-compatible presets for
Gemini, Groq, Mistral, xAI, DeepSeek, and OpenRouter. Their official compatible
base URLs are prefilled, while model discovery and manual model entry remain
available.

### Self-hosted defaults (`ollama`, `llamacpp`)

A server you run yourself is billed in memory and time rather than tokens, so
discovery configures these providers differently:

| Field | Default | Why |
| --- | --- | --- |
| `context_window` | `min(context_ceiling, limits.self_hosted_context_window)` — `32768` by default | The server allocates a KV cache this large *before* the first token. Providers report the architecture maximum (often 128k–256k); requesting it can take a modest host minutes, or push it into swap. |
| `context_ceiling` | reported maximum | Recorded, not requested — it is what the model *can* address, not a size worth allocating every turn. |
| `max_output_tokens` | `4096` | These tokens are not metered, and the general 1024 default truncates mid-answer. |
| `first_token_timeout_secs` | `600` | Loading a model and running prefill is not a stalled stream. `timeout_secs` stays the between-chunks stall timeout. |
| `keep_alive` (Ollama) | `30m` | Keeps the model resident so the next turn does not pay another cold load. |

#### Changing the window

An entry with `limit_mode = "auto"` is discovery's to manage: every refresh
settles it at `min(context_ceiling, limits.self_hosted_context_window)`, up or
down. So there are two ways to change it, and they mean different things.

**All self-hosted models at once** — you know how much memory the host has:

```sh
snx config set limits.self_hosted_context_window 131072
```

That writes the global override. Add `--workspace` to scope it to this
checkout instead, or `snx config reset limits.self_hosted_context_window` to
inherit the default again. The TUI equivalent names the scope explicitly:
`/config set global limits.self_hosted_context_window 131072`.

**One model, pinned** — take it out of discovery's hands:

```toml
[models.mistral_latest]
context_window = 65536
limit_mode = "manual"      # required, or the next refresh resets it
```

`snx config budgets` shows the effective value alongside the other limits.

Nothing here is a hard cap: the only ceiling snx enforces is the model's own
`context_ceiling`, because asking for more than the architecture addresses is
not a preference.

What the size actually costs, measured with `snx model test` against a 7B model
on a remote CPU-only host:

| Window | First token, cold | First token, resident |
| --- | --- | --- |
| 8192 | ~32 s | ~27 s |
| 32768 | ~216 s | ~34 s |

The whole cost is in the cold load, and `keep_alive = "30m"` means you pay it
once rather than per turn — which is why `first_token_timeout_secs` defaults to
600 rather than something that would trip on it. If your host is slower than
that, or has less memory to spare, lower the window; if it has a GPU or plenty
of RAM, raise it.

If a turn ends with *no first token after 600s*, the server is usually loading a
model too large for its memory. Check `snx model test <name>` — it reports
connection, first-token, and total latency separately — and prefer a model the
host can hold. An Ollama `unexpected EOF` means the model process died, which is
almost always the server running out of memory.

### Using GPT

OpenAI's API is OpenAI-compatible, so GPT works with the dedicated `openai`
provider. It defaults `base_url` to `https://api.openai.com/v1` and **requires**
an API key supplied by environment-variable name (never inline):

```toml
[models.gpt]
provider = "openai"
model = "gpt-4o"                 # or gpt-4.1, gpt-4o-mini, o4-mini, ...
api_key_env = "OPENAI_API_KEY"
role = "executor"
context_window = 128000
max_output_tokens = 4096
```

```sh
export OPENAI_API_KEY=sk-...      # the value is registered with the redactor at startup
snx catalog health                 # probes /v1/models with the bearer token
snx run "summarize this repo" --agent researcher
```

GPT supports native tool-calling, so the full typed-tool path is used. If the
key is missing, the provider fails fast with an actionable message rather than a
late 401. **Authentication is via API key** (see "Authentication" below).

Silent Nexus **never downloads models.** Run the server yourself and point a
`[models.*]` entry at it. `snx doctor` probes the default local ports
(llama.cpp `:8080`, Ollama `:11434`, OpenAI-compatible `:8000`) for detection
only.

## Routing and fallback

`[routing]` maps task classes to model names:

```toml
[routing]
simple   = "small"     # trivial classification/routing turns
coding   = "coder"
planning = "planner"
fallback = "coder"     # used when a routed model errors or times out
```

The deterministic classifier (`nexus-agent::classify`) picks the task class from
the objective — no model call needed — and `ModelManager` routes accordingly,
falling back on provider failure or timeout.

## Claude subscription planning

`claude-plan` delegates authentication and inference to the installed official
Claude CLI. NEXUS will not inspect or use an existing Claude login until the
operator explicitly consents through `/login claude-plan` or
`/auth use-existing-claude`.

The bridge launches one stateless print-mode process per request with
`--output-format stream-json`, `--safe-mode`, `--tools ""`,
`--permission-mode plan`, NEXUS-owned system instructions, one turn, and no
Claude session persistence. Claude cannot execute tools; it can only return
prose or the compatibility action schema, which re-enters the normal NEXUS
schema/policy/approval/sandbox pipeline.

```toml
[models.claude_plan]
provider = "claude-plan"
model = "sonnet"
context_window = 200000
max_output_tokens = 8192
```

Login itself is delegated to `claude auth login --claudeai`. Logout/revoking
consent pauses dependent background work before NEXUS stops using the profile.

## Native Anthropic API

`anthropic` implements the Messages API directly, including native tool-use
content blocks, tool-result messages, SSE streaming, usage reporting, model
discovery, and actionable error classification.

```toml
[models.anthropic]
provider = "anthropic"
model = "<anthropic-model-id>"
api_key_env = "ANTHROPIC_API_KEY"
context_window = 200000
max_output_tokens = 8192
```

Anthropic credentials use the `x-api-key` header plus the required
`anthropic-version` header. They are not treated as OpenAI bearer tokens.

## Compatible API presets

The interactive provider catalog supplies these base URLs:

| Preset | Base URL | Default key variable |
|---|---|---|
| Gemini | `https://generativelanguage.googleapis.com/v1beta/openai` | `GEMINI_API_KEY` |
| Groq | `https://api.groq.com/openai/v1` | `GROQ_API_KEY` |
| Mistral | `https://api.mistral.ai/v1` | `MISTRAL_API_KEY` |
| xAI | `https://api.x.ai/v1` | `XAI_API_KEY` |
| DeepSeek | `https://api.deepseek.com` | `DEEPSEEK_API_KEY` |
| OpenRouter | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` |

Keys entered through `/connect` are masked and stored in NEXUS credential
profiles; managed model configuration contains only the credential reference.

## Small-model / no-tool-call compatibility

Models without native tool-calling still work. When
`native_tool_calls = false` (or a provider reports no support), the harness
switches to a strict textual action protocol: the model is instructed to emit a
single JSON action object, which the compatibility parser
(`nexus-agent::action::parse_compat`) extracts and validates exactly like a
native tool call. Everything downstream — schema validation, policy, sandbox —
is identical.

## Secrets

API keys are referenced by environment-variable *name* (`api_key_env`), never
written inline in config. Their values are registered with the redactor at
startup so they cannot leak into logs, audit records, or output.

```toml
[models.remote]
provider    = "openai_compatible"
base_url    = "https://my-endpoint.example/v1"
model       = "my-model"
api_key_env = "MY_ENDPOINT_KEY"   # value read from the environment, never logged
role        = "executor"
```

The interactive `/connect` custom-endpoint form can instead store an optional
key in NEXUS's restricted credential store and write only an `api_key_ref`.
It accepts either `host:port` or a full URL, adds `/v1` for OpenAI-compatible
presets when no path is supplied, exposes HTTP/HTTPS and certificate
verification choices, and can test/model-discover before a model is selected.
TLS verification defaults on; disabling it is an explicit advanced choice for
a specifically trusted self-signed endpoint.

## Hardware acceleration (GPU detection)

Silent Nexus detects the host GPU and reports whether local models can be
accelerated. Detection is **pure and subprocess-free** — it reads sysfs/procfs
(Linux) and platform constants — so it is fast and honest: an absent or
unreadable GPU is reported as "none detected (CPU-only)", never guessed.

```sh
snx doctor          # shows a "gpu / accelerator" line (vendor, name, VRAM, backend)
snx catalog list     # adds an `accel` column: CUDA/ROCm/Metal/CPU for local, "remote" otherwise
snx catalog health   # appends the per-model accelerator; for Ollama, the REAL VRAM offload
```

What is detected:

- **Host GPU presence & vendor** — NVIDIA (→ CUDA), AMD (→ ROCm/Vulkan), Intel
  (→ oneAPI/Vulkan), Apple Silicon (→ Metal). NVIDIA model names come from
  `/proc`; AMD VRAM from sysfs. NVIDIA VRAM is left blank here (it needs
  `nvidia-smi`) rather than fabricated.
- **Per-model accelerator capability** — surfaced in `ModelCapabilities.accelerator`:
  `CUDA`/`ROCm`/`Metal`/… when a local model can use the host GPU, `CPU` when a
  local model has no GPU, and `null` for remote endpoints (whose hardware the
  harness cannot observe — so it does not pretend to).
- **Actual GPU offload (Ollama)** — `snx catalog health` queries Ollama's
  `/api/ps` and reports whether the loaded model sits in VRAM
  (`loaded on GPU (N MiB VRAM)`), is split, or is CPU-only. This is the honest,
  per-model answer to "is it really running on the GPU?". llama.cpp's API does
  not expose this, so it is not claimed for that provider.

Silent Nexus remains **CPU-first**: everything works without a GPU. Detection
simply lets you (and the agent, via `diag.system`) adapt — e.g. prefer a
smaller/quantized model on a CPU-only host.

## Authentication

Silent Nexus supports provider-specific authentication rather than pretending
all hosted APIs share one scheme. OpenAI-compatible providers use
`Authorization: Bearer <key>`; Anthropic uses `x-api-key`; Codex and Claude
subscription access are delegated to their official CLIs behind explicit
consent. Environment-variable names (`api_key_env`) and credential references
are persisted, never raw keys.

### Device / OAuth login via Codex ("Sign in with ChatGPT")

If you use OpenAI **Codex**, Silent Nexus uses an isolated Codex profile by
default. It can also detect the auth file written by the official `codex` CLI
in `~/.codex/auth.json` (or `$CODEX_HOME/auth.json`), but it will not read that
existing profile until you explicitly consent. Run `snx auth login` to choose
a login method:

```toml
[models.codex]
provider = "codex"
auth     = "codex"
model    = "<plan-model-id>"
role     = "executor"
# OAuth defaults to the ChatGPT Codex backend.
# API-key profiles default to https://api.openai.com/v1.
```

```sh
snx auth login              # interactive menu
snx auth login --device     # one-time device code for SSH/headless machines
snx auth login --api-key    # read and store an OpenAI API key
snx auth login --import     # copy an existing Codex login into isolated storage
snx auth login --use-existing # consent to read the existing profile in place
snx auth status             # shows session mode (oauth/api_key) + account - never the token
snx auth logout
```

Silent Nexus deliberately does **not** reimplement OpenAI's OAuth: login is
performed by the trusted, official `codex` CLI. `snx auth login --api-key` also
delegates storage to `codex login --with-api-key`, so Silent Nexus consumes the
same auth file shape as Codex itself. The token/key is registered with the
redactor at startup so it never appears in logs, audit records, or output.

An isolated Codex login is eligible during setup. A separate existing Codex CLI
login is only eligible after the consent flow above; mere detection is not
authentication. Missing or removed credentials do not prevent NEXUS from
starting—the provider is reported unavailable and `/connect` remains available
to repair it.

A Claude subscription login and an Anthropic API key are deliberately separate
providers. Likewise, there is no provider-agnostic OAuth for raw GPT API keys;
the Codex route delegates to Codex's own login instead of inventing one.

Local providers (llama.cpp, Ollama) need no authentication at all.
