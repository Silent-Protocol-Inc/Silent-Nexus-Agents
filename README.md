# Silent Nexus 1.0

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
  effect — `container`, `approval-only-host`, or `path-validation-only` — and
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

The certified 1.0.0 target is `x86_64-unknown-linux-gnu`. Rust development and
source builds use the pinned `1.97.0` toolchain and the committed lockfile.
Other operating systems and architectures remain experimental until they have
independent release evidence.

From source:

```sh
git clone https://github.com/silent-protocol/silent-nexus.git
cd silent-nexus
scripts/install.sh --user
snx doctor --deep
```

System installation:

```sh
cargo build --release --locked -p nexus-cli
sudo scripts/install.sh --system --binary target/release/snx
```

The installer also installs the man page and Bash, Zsh, and Fish completions.
Use `--prefix PATH` for a custom prefix, `--dry-run` to inspect changes, or
`--uninstall` to remove program files without deleting configuration or
workspace data.

For a packaged release, verify the adjacent `SHA256SUMS`, extract the archive,
run its internal `sha256sum -c SHA256SUMS`, and install the included `snx`.
Archives contain the license, README, man page, completions, SPDX SBOM, and an
internal SHA-256 manifest.

NEXUS does **not** download models or container images automatically. Point it
at a model server you operate, and explicitly pull the pinned sandbox image
before selecting container execution.

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
| `snx doctor [--deep]` | Diagnostics; deep mode adds integrity, permissions, isolation, release, and binary checks |
| `snx maintenance check` | Database, WAL, permission, migration, storage, and artifact integrity |
| `snx maintenance backup <directory>` | Atomic SQLite/artifact snapshot with a hash manifest |
| `snx maintenance optimize [--vacuum]` | Safe optimization/checkpoint; refuses active work |
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
nexus-sandbox      execution backends: strong container, approval-only host,
                   mock,
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
or command capabilities. The process backend is approval-only host execution,
not containment: every model-proposed terminal action needs a prominent
one-time attended approval, and unattended/background terminal execution is
denied. Automatic terminal execution requires the strong container backend.

Generic model filesystem tools cannot access `.nexus`, `.git`, common
credential paths, private keys, or credential stores. Generic terminal
privilege escalation and Git commit/push/remote/alias operations are denied;
local commits remain available through the audited typed workflow.

## Upgrades, rollback, and data

Before upgrading, run:

```sh
snx maintenance check
snx maintenance backup "$HOME/snx-backup-$(date +%Y%m%d)"
```

Install the new binary atomically, then run `snx doctor --deep`. To roll back,
restore the prior verified binary. Database migrations are append-only; a
binary older than the migrated state may not understand newer schema, so a
full rollback uses the backup made before upgrade.

Workspace data lives under `<workspace>/.nexus/state`; user configuration and
auth profiles use the platform configuration directory (on Linux,
`~/.config/silent-nexus`, subject to `XDG_CONFIG_HOME`). Silent Nexus 1.0 never
automatically deletes transcripts, goals, plans, tasks, memories, or artifacts.

## Documentation

- [`docs/configuration.md`](docs/configuration.md) — precedence and config groups
- [`docs/cli-reference.md`](docs/cli-reference.md) — public CLI surface
- [`docs/operator-guide.md`](docs/operator-guide.md) — safe daily operation
- [`docs/data-management.md`](docs/data-management.md) — state, backup, restore, retention
- [`docs/compatibility.md`](docs/compatibility.md) — the 1.x compatibility contract
- [`docs/upgrade-0.2-to-1.0.md`](docs/upgrade-0.2-to-1.0.md) — migration checklist
- [`docs/troubleshooting.md`](docs/troubleshooting.md) — diagnostic playbook
- [`docs/release-process.md`](docs/release-process.md) — reproducible release gates
- [`docs/support.md`](docs/support.md) and [`docs/governance.md`](docs/governance.md)

## License

Apache-2.0. See [`LICENSE`](LICENSE).
