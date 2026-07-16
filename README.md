# NEXUS

```
  ▚  N  E  X  U  S
     by Silent Protocol
  LOCAL INTELLIGENCE. CONTROLLED EXECUTION.
```

NEXUS (`snx`), by Silent Protocol, is a **local-first, model-agnostic agentic CLI harness**
written in Rust. It lets local and hosted models — llama.cpp, Ollama, Codex,
Claude subscriptions, Anthropic, or OpenAI-compatible APIs — do real work on
your machine while keeping every dangerous capability on the harness side of a
hard boundary the model cannot cross.

The design premise is inverted from most agent frameworks: the model is treated
as an **untrusted planner**, not a trusted operator. It proposes actions; the
harness validates, authorizes, sandboxes, executes, and verifies them. Safety
does not depend on the model's competence or good faith.

## What makes it different

- **The model never holds the keys.** Schema validation, capability checks,
  workspace confinement, policy/approval, sandboxing, timeouts, output caps,
  secret redaction, and audit logging all live in Rust, outside the model's
  reach. A jailbroken or malicious model still cannot escape them.
- **Honest sandboxing.** NEXUS tells you the *actual* isolation level in
  effect — `container`, `process-restricted`, or `path-validation-only` — and
  never claims strong isolation it isn't providing.
- **Evidence-based goals.** A `/goal` only completes when every acceptance
  criterion has recorded, tool-sourced evidence — never because the model said
  "done." Goals are durable SQLite state and survive restarts.
- **Small-model first.** Deterministic task classification, lazy tool
  discovery, minimal tool subsets, and a compatibility layer for models without
  native tool-calling keep weak local models on the rails. CPU-only is a
  first-class target.
- **Durable execution timeline.** Messages, routing, plans, reasoning summaries,
  approvals, tool lifecycle, diffs, validation, retries, tasks, subagents,
  checkpoints, and final answers form one paged, searchable timeline. Running
  cards stream in place; full artifacts load only when requested.
- **Truthful active work.** The context rail reports the actual objective,
  provider/model, plan stage, foreground tool, tasks, subagents, Git changes,
  validation, approvals, interruptions, and context-window usage.
- **Durable interactive work.** Sessions retain provider token usage, tool
  calls, elapsed runtime, titles, goals, personas, approved profile traits,
  view state, and linked continuation checkpoints. Exit handoffs print exact
  `snx resume <id>` and `snx continue <id>` commands after restoring the
  terminal.
- **Customizable, never overrideable.** Personas, profiles, retrieved memory,
  connectors, and proposed skills can shape behavior, but immutable safety,
  project policy, sandbox restrictions, and provider-required instructions
  always take precedence.
- **Web content is data, never instructions.** Fetched pages are wrapped as
  untrusted input; SSRF, private-range, cloud-metadata, and DNS-rebinding
  protections are enforced in the harness.
- **No fakes.** There are no placeholder handlers, simulated sandboxes, or
  hardcoded model replies anywhere in this codebase. If a capability is
  advertised, it is real; if a limitation exists, it is stated.

## Install

Requires Rust stable (1.97+).

```sh
git clone <repo> silent-nexus && cd silent-nexus
cargo build --release
./target/release/snx doctor
```

Or use the helper:

```sh
scripts/install.sh          # builds release and copies snx to ~/.local/bin
```

NEXUS does **not** download models. Point it at a local server you run
yourself (see below).

## Quick start

```sh
# 1. Onboard: detect installed local models (Ollama / llama.cpp) + GPU and
#    write a ready-to-use config. Works in any folder.
snx setup

#    (No local runtime? snx setup tells you how to install one, or use
#     `snx auth login --device` for headless Codex auth, or `--api-key`.)

# 2. Check readiness — models, GPU/accelerator, sandbox isolation, local servers
snx doctor

# 3. Interactive TUI (use --inline / --no-alt-screen for terminal scrollback)
snx

# 4. Or one-shot, non-interactive
snx run "summarize the architecture of this repo" --agent researcher
```

## Command surface

| Command | Purpose |
|---|---|
| `snx setup` | First-run onboarding: detect local models + GPU, write a config |
| `snx` / `snx chat` | Full-screen NEXUS TUI |
| `snx --inline` | TUI without the alternate screen, preserving native terminal scrollback |
| `snx run <objective>` | Run one objective to completion |
| `snx init` | Detect project instructions or preview/confirm a starter `AGENTS.md` |
| `snx resume <id>` | Launch the TUI directly on a session or recoverable goal |
| `snx summary [--session <id>]` | Save/copy a structured handoff and optionally create a linked rollover |
| `snx goal …` | Create / list / show / verify durable goals |
| `snx session …` | Inspect sessions or persist a title |
| `snx persona …` | Create, inherit, clone, select, and review persona definitions |
| `snx profile …` | Select profiles and review explicit/inferred workflow traits or RSI proposals |
| `snx memory …` | Approval-gated, secret-refusing long-term memory |
| `snx skill …` | Versioned, inspectable skills |
| `snx mcp …` | MCP client (register/connect) and server (`serve`) |
| `snx connector …` | Discover/preview/import Codex MCP and Agent Skill definitions, disabled/untrusted |
| `snx branch …` | Local status/diff/stage/unstage/restore/log/branch workflows |
| `snx commit -m <message> -f <path>…` | Preview selected files, confirm, and commit only those files |
| `snx sandbox status\|test` | Inspect and self-test the execution sandbox |
| `snx index …` | Build/query the code-intelligence index |
| `snx tools …` | List tools and their risk levels |
| `snx models …` | List models / probe provider health |
| `snx auth …` | Consent-gated Codex/Claude CLI auth and stored provider credentials |
| `snx config show\|path\|schema` | Configuration inspection |
| `snx audit` | Recent audit events |
| `snx doctor` | Environment and readiness diagnostics |
| `snx completion <shell>` | Shell completion script |

`--json` gives machine-readable output on most commands; `--no-color` (and
`NO_COLOR`) disable all styling.

Inside the TUI, `/details compact|expanded|raw` controls card density;
`/transcript` filters messages, plans, tools, diffs, agents, warnings, or
errors; `/context` inspects the exact redacted provider-request manifest; and
`/export markdown|jsonl` exports the durable event stream. Ctrl+F searches,
`n`/`N` navigate matches, Enter expands a card or lazily opens its artifact,
F6 cycles input/timeline/context/agent focus, and the arrow drawers expose live
context and agent/session activity on smaller terminals.

Argumentless configuration commands open menus, including `/model`, `/agent`,
`/permissions`, `/persona`, `/profile`, `/connector`, `/theme`, and
`/thinking`. `/plan`, `/task`, `/subagents`, and `/continue` manage durable
work. Shift+Tab cycles the visible approval mode through
`read-only → default → auto-edit → full-access`; destructive and external
actions still require one-time approval. `/btw` remains concurrent and
read-only.

## Architecture at a glance

NEXUS is a Cargo workspace of focused crates:

```
nexus-core         safety primitives: workspace guard, redaction, sanitize,
                   risk levels, config, storage, audit events, artifacts
nexus-policy       layered policy + approval engine (allow/ask/deny)
nexus-models       model providers (llama.cpp/Ollama/Codex/Claude Plan/
                   Anthropic/OpenAI-compatible/mock), routing and streaming
nexus-sandbox      execution backends: container, restricted-process, mock,
                   each reporting honest isolation
nexus-tools        typed tools (fs, repo, terminal+PTY, web, diagnostics)
nexus-agent        controlled streaming loop, plans/stages, custom agents,
                   bounded tasks and specialized subagents
nexus-goals        durable, evidence-verified goal engine
nexus-memory       long-term memory (refuses secrets, approval-gated)
nexus-context      context-window packing and safe compaction
nexus-index        AST/heuristic symbol index for small-model grounding
nexus-skills       versioned, payload-free skill packages
nexus-mcp          Model Context Protocol client and server
nexus-observability structured logging + audit log
nexus-cli          the `snx` binary
nexus-tui          the ratatui NEXUS interface
```

See [`docs/architecture.md`](docs/architecture.md) for the full per-turn
pipeline and [`docs/threat-model.md`](docs/threat-model.md) for what Silent
Nexus does and does not defend against.

## Security posture

NEXUS prefers **security over autonomy** and **honest limitations over
fake capabilities**. Read [`SECURITY.md`](SECURITY.md) and
[`docs/sandbox-security.md`](docs/sandbox-security.md) before granting it write
or command capabilities. In particular: the restricted-process backend is
*not* a container and does not hide host paths or the kernel attack surface —
`snx sandbox status` always tells you what you actually have.

## License

Apache-2.0. See [`LICENSE`](LICENSE).
