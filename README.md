<div align="center">

```
             ▚  N  E  X  U  S
            by Silent Protocol
```

# Silent Nexus

### Local intelligence. Controlled execution.

**A local-first, model-agnostic agentic CLI harness — where the model is an untrusted planner, not a trusted operator.**

[![CI](https://github.com/Silent-Protocol-Inc/Silent-Nexus-Agents/actions/workflows/ci.yml/badge.svg)](https://github.com/Silent-Protocol-Inc/Silent-Nexus-Agents/actions/workflows/ci.yml)
[![Security](https://github.com/Silent-Protocol-Inc/Silent-Nexus-Agents/actions/workflows/security.yml/badge.svg)](https://github.com/Silent-Protocol-Inc/Silent-Nexus-Agents/actions/workflows/security.yml)
[![Release](https://img.shields.io/badge/release-v2.0.0-success.svg)](https://github.com/Silent-Protocol-Inc/Silent-Nexus-Agents/releases/latest)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.97.0-orange.svg?logo=rust)](rust-toolchain.toml)
[![Platform](https://img.shields.io/badge/platform-x86__64--linux-lightgrey.svg)](#install)

[Install](#install) · [Quick start](#quick-start) · [Commands](#command-surface) · [Architecture](#architecture-at-a-glance) · [Security](#security-posture) · [Docs](#documentation)

</div>

---

**NEXUS** (`snx`), by Silent Protocol, lets local and hosted models — llama.cpp,
Ollama, Codex, Claude subscriptions, Anthropic, or any OpenAI-compatible API — do
real work on your machine while keeping every dangerous capability on the harness
side of a hard boundary the model cannot cross.

The design premise is inverted from most agent frameworks: the model is treated
as an **untrusted planner**, not a trusted operator. It proposes actions; the
harness validates, authorizes, sandboxes, executes, and verifies them. **Safety
does not depend on the model's competence or good faith.** A jailbroken or
malicious model still cannot escape the boundary.

```
   model proposes  ─►  schema validation ─► capability / role gate ─► workspace
   confinement ─► policy / approval ─► sandbox ─► timeout & output cap ─►
   secret redaction ─► sanitize ─► audit log  ─►  result verified against evidence
```

## Contents

- [What makes it different](#what-makes-it-different)
- [Supported models & providers](#supported-models--providers)
- [Install](#install)
- [Quick start](#quick-start)
- [Command surface](#command-surface)
- [Architecture at a glance](#architecture-at-a-glance)
- [Security posture](#security-posture)
- [Upgrades, rollback, and data](#upgrades-rollback-and-data)
- [Documentation](#documentation)
- [Contributing](#contributing)
- [License](#license)

## What makes it different

- **The model never holds the keys.** Schema validation, capability checks,
  workspace confinement, policy/approval, sandboxing, timeouts, output caps,
  secret redaction, and audit logging all live in Rust, outside the model's
  reach. A jailbroken or malicious model still cannot escape them.
- **Honest sandboxing.** NEXUS reports the *actual* isolation level in effect —
  `container`, `approval-only-host`, or `path-validation-only` — and never claims
  strong isolation it isn't providing.
- **Evidence-based goals.** A `/goal` only completes when every acceptance
  criterion has recorded, tool-sourced evidence — never because the model said
  "done." Goals are durable SQLite state and survive restarts.
- **Small-model first.** Deterministic task classification, lazy tool discovery,
  minimal tool subsets, and a compatibility layer for models without native
  tool-calling keep weak local models on the rails. CPU-only is a first-class
  target.
- **Adaptive harness.** Constrained models (small context, no native tools, or no
  structured output) automatically get smaller validated plan stages, a truncated
  tool surface, and clamped step budgets. A dependency-aware background scheduler
  with exclusive resource claims parks dependents of failed work and self-heals.
- **Durable execution timeline.** Messages, routing, plans, reasoning summaries,
  approvals, tool lifecycle, diffs, validation, retries, tasks, subagents,
  checkpoints, and final answers form one paged, searchable timeline. Running
  cards stream in place; full artifacts load only when requested.
- **Truthful active work.** The context rail reports the actual objective,
  provider/model, plan stage, foreground tool, tasks, subagents, Git changes,
  validation, approvals, interruptions, and context-window usage.
- **Durable interactive work.** Sessions retain provider token usage, tool calls,
  elapsed runtime, titles, goals, personas, approved profile traits, view state,
  and linked continuation checkpoints. Exit handoffs print exact
  `snx resume <id>` and `snx continue <id>` commands after restoring the terminal.
- **Customizable, never overrideable.** Personas, profiles, retrieved memory,
  connectors, and proposed skills can shape behavior, but immutable safety,
  project policy, sandbox restrictions, and provider-required instructions always
  take precedence.
- **Flagship agent: `nexus`, a Recursive Self-Improvement (RSI) generalist.** The
  default agent on a fresh install plans, implements, verifies, and delegates, and
  it improves over time: finished turns are mined for reusable workflows, repeated
  failures, and stated preferences, each recorded as an *approval-gated* proposal
  you review with `snx profile`. Nothing is ever applied without your approval.
  Tune or disable the analysis under `[self_improvement]`
  (`enabled = false` turns it off); `snx status` shows any pending proposals.
- **Web content is data, never instructions.** Fetched pages are wrapped as
  untrusted input; SSRF, private-range, cloud-metadata, and DNS-rebinding
  protections are enforced in the harness.
- **No fakes.** There are no placeholder handlers, simulated sandboxes, or
  hardcoded model replies anywhere in this codebase. If a capability is
  advertised, it is real; if a limitation exists, it is stated.

## Supported models & providers

NEXUS is model-agnostic. Point it at a server you operate or an account you own —
it never downloads models or container images on your behalf.

| Provider | Kind | Notes |
|---|---|---|
| **llama.cpp** | Local | OpenAI-compatible server; CPU-first, GPU-aware |
| **Ollama** | Local | Auto-detected via `/api/tags`; reports real VRAM offload |
| **OpenAI-compatible** | Local / hosted | Any `/v1` endpoint (LM Studio, vLLM, text-generation-webui, …) |
| **Codex (ChatGPT plan)** | Hosted | OAuth via the official `codex` CLI session; plan-model discovery |
| **Claude subscription** | Hosted | Reuses the official Claude CLI auth session |
| **Anthropic API** | Hosted | Direct API key |
| **OpenAI API** | Hosted | Direct API key |
| **mock** | Test | Deterministic replies for CI and offline development |

Deterministic routing, fallback chains, and a no-tool-call compatibility layer
mean a weak local model and a frontier hosted model run through the same harness
with the same guarantees. See [`docs/providers.md`](docs/providers.md).

## Install

Release packaging and gates target **`x86_64-unknown-linux-gnu`**. Source builds
use the pinned **Rust 1.97.0** toolchain and the committed lockfile. Certification
is established only by a completed release report; other operating systems and
architectures remain experimental until they have independent release evidence.

**From source:**

```sh
git clone https://github.com/Silent-Protocol-Inc/Silent-Nexus-Agents.git
cd Silent-Nexus-Agents
scripts/install.sh --user
snx doctor --deep
```

**System-wide:**

```sh
cargo build --release --locked -p nexus-cli
sudo scripts/install.sh --system --binary target/release/snx
```

**From a packaged release** ([latest](https://github.com/Silent-Protocol-Inc/Silent-Nexus-Agents/releases/latest)):

```sh
# Verify, extract, re-verify the internal manifest, then install the binary
sha256sum -c SHA256SUMS
tar -xzf silent-nexus-2.0.0-x86_64-unknown-linux-gnu.tar.gz
cd silent-nexus-2.0.0-x86_64-unknown-linux-gnu
sha256sum -c SHA256SUMS
install -m 0755 snx ~/.local/bin/snx
```

The installer also installs the man page and Bash, Zsh, and Fish completions.
Use `--prefix PATH` for a custom prefix, `--dry-run` to inspect changes, or
`--uninstall` to remove program files without deleting configuration or workspace
data. Archives contain the license, README, man page, completions, an SPDX SBOM,
and an internal SHA-256 manifest.

> NEXUS does **not** download models or container images automatically. Point it
> at a model server you operate, and explicitly pull the pinned sandbox image
> before selecting container execution.

## Quick start

```sh
# 1. Onboard: detect installed local models (Ollama / llama.cpp) + GPU and
#    write a ready-to-use config. Works in any folder.
snx setup
#    No local runtime? snx setup tells you how to install one, or use
#    `snx auth login --device` for headless Codex auth, or `--api-key`.

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
| `snx catalog …` | List models / probe provider health |
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
`/transcript` filters messages, plans, tools, diffs, agents, warnings, or errors;
`/context` inspects the exact redacted provider-request manifest; and
`/export markdown|jsonl` exports the durable event stream. Ctrl+F searches,
`n`/`N` navigate matches, Enter expands a card or lazily opens its artifact, F6
cycles input/timeline/context/agent focus, and the arrow drawers expose live
context and agent/session activity on smaller terminals.

Argumentless configuration commands open menus, including `/model`, `/agent`,
`/permissions`, `/persona`, `/profile`, `/connector`, `/theme`, and `/thinking`.
`/plan`, `/task`, `/subagents`, and `/continue` manage durable work. Shift+Tab
cycles the visible approval mode through
`read-only → default → auto-edit → full-access`; destructive and external actions
still require one-time approval. `/btw` remains concurrent and read-only.

## Architecture at a glance

NEXUS is a Cargo workspace of 16 focused crates (~68k LOC, 458 tests):

```
nexus-core          safety primitives: workspace guard, redaction, sanitize,
                    risk levels, config, storage, audit events, artifacts
nexus-policy        layered policy + approval engine (allow / ask / deny)
nexus-models        model providers (llama.cpp / Ollama / Codex / Claude plan /
                    Anthropic / OpenAI-compatible / mock), routing and streaming
nexus-sandbox       execution backends: strong container, approval-only host,
                    mock — each reporting honest isolation
nexus-tools         typed tools (fs, repo, terminal + PTY, web, diagnostics)
nexus-agent         controlled streaming loop, plans / stages, custom agents,
                    bounded tasks and specialized subagents
nexus-goals         durable, evidence-verified goal engine
nexus-memory        long-term memory (refuses secrets, approval-gated, FTS5)
nexus-context       context-window packing and safe compaction
nexus-index         AST / heuristic symbol index for small-model grounding
nexus-skills        versioned, payload-free skill packages
nexus-mcp           Model Context Protocol client and server
nexus-app           shared service layer: one command registry + services
                    powering both the CLI and the TUI (surfaces cannot drift)
nexus-observability structured logging + audit log
nexus-cli           the `snx` binary
nexus-tui           the ratatui NEXUS interface
```

See [`docs/architecture.md`](docs/architecture.md) for the full per-turn pipeline
and [`docs/threat-model.md`](docs/threat-model.md) for what Silent Nexus does and
does not defend against.

## Security posture

NEXUS prefers **security over autonomy** and **honest limitations over fake
capabilities**. Read [`SECURITY.md`](SECURITY.md) and
[`docs/sandbox-security.md`](docs/sandbox-security.md) before granting it write or
command capabilities. The process backend is approval-only host execution, not
containment: every model-proposed terminal action needs a prominent one-time
attended approval, and unattended/background terminal execution is denied.
Automatic terminal execution requires the strong container backend.

Generic model filesystem tools cannot access `.nexus`, `.git`, common credential
paths, private keys, or credential stores. Generic terminal privilege escalation
and Git commit/push/remote/alias operations are denied; local commits remain
available through the audited typed workflow.

To report a vulnerability, follow the process in [`SECURITY.md`](SECURITY.md) —
please do not open a public issue for security reports.

## Upgrades, rollback, and data

Before upgrading, run:

```sh
snx maintenance check
snx maintenance backup "$HOME/snx-backup-$(date +%Y%m%d)"
```

Install the new binary atomically, then run `snx doctor --deep`. To roll back,
restore the prior verified binary. Database migrations are append-only; a binary
older than the migrated state may not understand newer schema, so a full rollback
uses the backup made before upgrade.

Workspace data lives under `<workspace>/.nexus/state`; user configuration and auth
profiles use the platform configuration directory (on Linux,
`~/.config/silent-nexus`, subject to `XDG_CONFIG_HOME`). Silent Nexus 2.x never
automatically deletes transcripts, goals, plans, tasks, memories, or artifacts.
See the [`CHANGELOG`](CHANGELOG.md) for release-by-release detail.

## Documentation

| Guide | Contents |
|---|---|
| [`docs/architecture.md`](docs/architecture.md) | Full per-turn pipeline and crate boundaries |
| [`docs/threat-model.md`](docs/threat-model.md) | What NEXUS does and does not defend against |
| [`docs/sandbox-security.md`](docs/sandbox-security.md) | Isolation backends and their honest guarantees |
| [`docs/providers.md`](docs/providers.md) | Model providers, routing, and fallback |
| [`docs/configuration.md`](docs/configuration.md) | Precedence and configuration groups |
| [`docs/cli-reference.md`](docs/cli-reference.md) | Public CLI surface |
| [`docs/operator-guide.md`](docs/operator-guide.md) | Safe daily operation |
| [`docs/goals.md`](docs/goals.md) · [`docs/memory-and-skills.md`](docs/memory-and-skills.md) · [`docs/mcp.md`](docs/mcp.md) | Feature deep-dives |
| [`docs/data-management.md`](docs/data-management.md) | State, backup, restore, retention |
| [`docs/compatibility.md`](docs/compatibility.md) | The 1.x compatibility contract |
| [`docs/upgrade-0.2-to-1.0.md`](docs/upgrade-0.2-to-1.0.md) | Migration checklist |
| [`docs/troubleshooting.md`](docs/troubleshooting.md) | Diagnostic playbook |
| [`docs/release-process.md`](docs/release-process.md) | Reproducible release gates |
| [`docs/support.md`](docs/support.md) · [`docs/governance.md`](docs/governance.md) | Support and governance |

## Contributing

Contributions are welcome. Please read [`CONTRIBUTING.md`](CONTRIBUTING.md),
[`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md), and [`AGENTS.md`](AGENTS.md) first —
they cover the build environment, coding standards, community expectations, and
the write-discipline rules the codebase enforces. Every change must pass the
full local gate before review:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
```

CI additionally runs documentation, `cargo audit`, `cargo deny`, a secret scan,
and release-archive validation. See [`docs/release-process.md`](docs/release-process.md).

## License

Licensed under the **Apache License 2.0** — Copyright © 2026 **Silent Protocol**.
See [`LICENSE`](LICENSE) for the full text and [`NOTICE`](NOTICE) for
attribution. Silent Nexus (NEXUS / `snx`) is a product of the Silent Protocol
brand.

Contributions are accepted under the same Apache-2.0 terms; see
[`CONTRIBUTING.md`](CONTRIBUTING.md). All participation is governed by our
[`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md).

<div align="center">
<sub><b>NEXUS</b> · by Silent Protocol · Local intelligence. Controlled execution.<br>
© 2026 Silent Protocol · Apache-2.0</sub>
</div>
