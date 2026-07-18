# Changelog

All notable changes to NEXUS are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project adheres to
Semantic Versioning.

## [1.1.2] — 2026-07-18

Patch release. Makes file changes visible in the timeline: file-mutating tool
calls now render a diff card with the file path and highlighted `+`/`-` lines.
No schema changes, no new commands.

### Fixed

- Creating a file (`fs.create_file`) now shows a timeline diff card with the
  file path as a header and every new line highlighted as an added (`+`) line.
  Previously the timeline only showed a one-line "wrote …" summary with no diff
  and no path, because a diff card was emitted solely when the tool name
  contained "diff" or the output was a git patch.
- `fs.patch_file` shows the replaced text as removed (`-`) and the new text as
  added (`+`); `fs.delete` shows the removed file's contents as `-` lines; and
  `fs.move` shows the destination path. Each card carries insertion/deletion
  counts.
- The timeline diff card now renders the file path and colorizes `+`/`-`/`@@`
  lines in every surface (TUI transcript and CLI run output). Previously the
  `TimelineKind::Diff` body had no renderer and only appeared as raw JSON when a
  card was expanded, so git-diff cards also showed no path or colors.

### Changed

- Structured tool diffs travel in tool-output metadata (never in the
  model-facing content), so richer diff cards add no model-context cost.

## [1.1.1] — 2026-07-18

Patch release from a post-1.1.0 stability audit (eight-angle diff review with
per-finding verification). Fixes ten confirmed correctness, privacy, and
convention issues; no schema changes, no new commands.

### Fixed

- `/memory show <id>` no longer returns the content of a forgotten memory.
  `forget` soft-deletes (status `deleted`) so the legacy-import dedup can still
  see the row, but the by-id lookup now rejects deleted rows instead of
  rendering their full payload — a privacy regression versus 1.0's hard delete.
- `/plan pause` and `/plan resume` no longer clobber non-runnable task states.
  Only `Draft`/`Pending`/`Ready`/`Running` tasks pause, and only `Paused` tasks
  resume, so pause/resume can no longer resurrect a `Failed` task or bypass a
  `Waiting`-on-approval gate.
- `/improve apply|rollback` on a skill proposal now takes the atomic status
  transition before toggling the skill, and restores the prior status if the
  skill toggle fails, so concurrent apply/rollback can no longer leave the
  skill's enabled state decoupled from the recorded proposal status.
- `/memory approve` and `/memory reject` are now blocked while a turn is active,
  matching the other memory mutations, so they cannot race the running turn's
  own memory writes. Read-only memory subcommands still run mid-turn.
- `/profile` operations (report, review, delete-fact, rename, export) resolve
  the canonical profile on demand when a background turn established the session
  context before a profile was set, instead of failing with "no active
  profile". Resolution does not rewrite the turn context, so prompt composition
  is unchanged.
- `/resume` distinguishes provider availability from model availability: a
  configured model whose provider credential is missing/revoked now recommends
  re-authentication or a model/provider switch instead of reporting the
  environment as an exact match. (Still a synchronous credential check, not a
  live reachability probe.)
- Non-interactive `/connect` now reports local runtimes and configured
  endpoints (matching the interactive menu) instead of the hosted-auth catalog
  that belongs to `/login`.
- A dependency-parked background task (`blocked`) is no longer mislabeled as
  `waiting_approval` in session snapshots and continuation checkpoints; it
  re-queues itself once its dependency clears.
- Dependency-block detection shares a single sentinel constant between the
  writer and the auto-requeue matcher, and `retry_task` now accepts `blocked`
  tasks so an operator has a manual escape hatch if a task is ever stranded.
- `/memory export` and `/profile export` write via
  `nexus_core::atomic::atomic_write_private` (O_NOFOLLOW, same-directory atomic
  replace, `0600`) per the AGENTS.md write discipline, instead of a bare
  `std::fs::write` that followed symlinks and left default permissions.

## [1.1.0] — 2026-07-17

Silent Nexus 1.1 is the adaptive-harness release line. Automated gates
(fmt, clippy, tests, docs, audit, deny, secret scan), release packaging and
archive validation, and live remote-Ollama scenarios recorded evidence on
2026-07-17; see `docs/adaptive-harness-delivery-report.md`.

### Added (completion session, 2026-07-17)

- Background scheduler honors task dependencies: `background_task_dependencies`
  (migration `0007_task_dependencies`) gates leasing, parks dependents of
  failed/cancelled prerequisites as `blocked` with a diagnostic error, and
  self-heals them back to `queued` once every dependency completes. Cycles and
  self-edges are rejected transactionally.
- Writer background tasks claim the git repository through
  `harness_resource_claims` before creating a worktree; conflicting writers
  are parked `blocked` instead of racing, and claims release on drop or lease
  expiry.
- `/resume` validates the latest checkpoint before reattaching: environment
  fingerprint, per-file content hashes, model availability, and stale
  assumptions are re-checked via `assess_recovery`, and the recovery report is
  rendered in both the TUI attach flow and the CLI resume path.
- Weak-model adaptation: `ModelCapabilities::constrained()` (small context,
  no native tool calls, or no structured output) shrinks the planned
  decomposition before the plan is recorded
  (`WorkEstimate::constrained_for_weak_model`), truncates the tool surface,
  and clamps per-turn step/repetition budgets.
- `/agent show <role>` (capability card) and `/agent recommend <objective>`
  (deterministic classifier-based suggestion; never auto-switches).
- Command-surface completion: `/memory scopes|stats|candidates|contradictions|export`,
  `/task graph|depend|validate|assign`, `/subagents limits`,
  `/goal archive|risks`, `/persona show|reset`, `/profile rename|export`, and
  a top-level `/improve` command (list/show/approve/reject/apply/rollback)
  with status-gated apply/rollback over RSI proposals.

### Fixed (validation session, 2026-07-17)

- `snx memory forget` now accepts `--yes` so non-interactive runs can
  authorize deletion, matching its own hint and `snx profile delete`.
- Canonical memory retrieval ranks records with a deterministic
  objective-overlap score (`canonical_memory_score`); the previous tree
  referenced the function without defining it and did not compile.
- Removed a parallel-test race on process-global `GIT_CONFIG_*` variables and
  re-baselined the timeline render snapshots after visual inspection.

### Release scope (validated 2026-07-17)

- One bounded, persisted harness context linking profiles, scoped memory,
  system-prompt personas, agents, goals, plans, task graphs, subagents,
  provider/model selection, evaluation, checkpoints, and improvement
  proposals.
- Menu-first slash-command control surfaces backed by canonical domain
  services rather than display-only actions.
- Provider-neutral model request/response/reference contracts and normalized
  capability, privacy, locality, cost, latency, and fallback metadata.
- Duplicate prompt/answer and first-line rendering corrections, including
  turn-scoped terminal-event idempotency.
- Append-only `0006_adaptive_harness` and `0007_task_dependencies` migrations
  while configuration schema version remains `1` throughout the compatible
  1.x line.

## [1.0.0] — 2026-07-17

First production-certified Silent Nexus release for
`x86_64-unknown-linux-gnu`.

### Added

- Structured command analysis across shell chains, wrappers, interpreters, and
  substitutions, with hard denials for privilege escalation and generic
  terminal Git mutation bypasses.
- Explicit isolation strength and filesystem-access metadata. Container actions
  run as the invoking UID/GID with per-action read-only/write mounts,
  sensitive-path masks, network-off defaults, dropped capabilities, resource
  limits, and a digest-pinned image.
- Append-only `0005_production_hardening` migration with migration checksums,
  timeline FTS, status indexes, and backward-compatible FTS backfill.
- `snx maintenance check`, `backup`, and `optimize`, plus `snx doctor --deep`.
- Atomic private writes, permission repair, zeroized secret buffers, verified
  artifact reads, bounded/sanitized Git subprocesses, SQLite busy handling, and
  one shared stdout/stderr kill budget.
- Deterministic Linux release packaging with man page, shell completions, SPDX
  SBOM, internal/external SHA-256 manifests, CI/security/release workflows, and
  user/system installer modes.

### Changed

- Version and embedded release metadata are now `1.0.0`; the pinned Rust/MSRV
  is `1.97.0`, locked builds are required, and internal crates are
  non-publishable.
- Automatic model terminal execution requires strong container isolation.
  Host-process fallback is prominently reported as approval-only and is denied
  for unattended/background work.
- Generic model filesystem access now excludes `.nexus`, `.git`, common
  credential paths, private keys, and credential stores while preserving
  documented public examples such as `.env.example`.
- Transcript filtering pages until the requested match count is reached, while
  durable search uses SQLite FTS and loads the matching event's surrounding
  page. TUI rendering caches wrapped layouts and renders the visible range.

### Security

- Raw shell, interpreters, wrappers, substitutions, unrecognized commands, and
  unsafe host execution cannot receive session grants or auto-edit approval.
- Generic terminal `git commit`, `git push`, `git remote`, Git aliases,
  unrecognized Git subcommands, and privilege escalation are hard denied.
- Output-cap breaches terminate process groups or containers immediately,
  independent of command timeout.
- State/auth/log trees are repaired to private permissions; symlink and
  artifact-tampering attacks are rejected.
- Sensitive-path discovery, filesystem listings, and model-facing Git
  status/diff fail closed so denied credential paths cannot leak through
  metadata or repository output.

### Compatibility

- Config remains version `1`; migrations are append-only; existing timeline and
  redacted JSONL export fields remain compatible throughout 1.x except where a
  necessary security break is documented.
- Silent Nexus 1.0 does not automatically delete transcripts, tasks, plans,
  goals, memories, or artifacts.

## [0.2.0] — 2026-07-17

Cyberdeck transcript and agent-harness upgrade.

### Added

- Durable, typed execution timelines with lifecycle spans, redacted payloads,
  stable streamed cards, lazy artifacts, legacy-session projection, wrapped
  pagination, filtering, search, Markdown/JSONL export, and inline mode.
- Truthful active-work snapshots, request context manifests, complexity-aware
  work breakdowns with runtime promotion, versioned plans/stages/evidence,
  durable tasks, and agent-run state.
- Compact/expanded/raw transcript details, transcript filters, context
  inspection, focus/drawer controls, continuation checkpoints, provider
  presets, additional/custom agents, and eight accessible cyberdeck themes.
- Consent-gated official Claude CLI plan provider, native Anthropic Messages
  provider, and Gemini/Groq/Mistral/xAI/DeepSeek compatible presets.
- On-demand workspace worker with SQLite leases, stale-run recovery, three
  readers/one writer, and persistent external `snx/task/<id>` Git worktrees.
- Advanced `/plan`, `/task`, `/subagents`, `/continue`, `/details`,
  `/transcript`, `/context`, and `/export` command families.

### Fixed

- Cancellation closes running assistant/tool cards instead of leaving phantom
  activity in resumed transcripts.
- Continuation children clone the current plan/stage/evidence state and share
  rollover-root write idempotency, preventing completed parent writes from
  replaying under a new session id.
- Subagent cancel/retry updates the linked task, delegation is limited to
  audited orchestrators, root fan-out is capped at eight, and late worker
  completions cannot overwrite a newer pause/cancel state.
- Writer worktrees derive the true Git top-level and remain outside the source
  checkout even when NEXUS was invoked from a nested directory.
- Provider reset timestamps retain their original case, and context category
  token counts remain explicitly estimated even after a provider reports the
  request total.

## [0.1.1] — 2026-07-16

Correctness hotfix installed before the 0.2 orchestration redesign.

### Fixed

- Normalize every exposed and historical Codex tool name to the provider wire
  contract, including deterministic collision handling and reverse mapping.
- Validate the complete serialized Codex request locally before any HTTP
  request is sent.
- Surface deterministic HTTP 4xx failures without retrying an unchanged
  request.
- Treat non-empty prose as a completed compatibility turn and retry malformed
  action JSON once with a concise schema correction.
- Retain the post-0.1.0 authentication, credential, goal/session,
  configuration, logout, staged-file, instruction-file, atomic-state, and
  destructive-memory correctness fixes documented below.

## [0.1.0] — 2026-07-11

Initial release: a complete, real, production-grade agentic CLI harness.

### Added

- **Controlled agent loop** (`nexus-agent`): deterministic classification,
  minimal tool selection, schema-validated actions, policy/approval, sandboxed
  execution, independent verification, and bounded recovery — with a
  compatibility protocol for models lacking native tool-calling.
- **Safety core** (`nexus-core`): workspace confinement with symlink-swap
  protection, secret redaction, terminal sanitization, risk levels, layered
  config, SQLite store (WAL, 0600), audit events, content-addressed artifacts.
- **Policy engine** (`nexus-policy`): layered allow/allow_session/ask/deny with
  builtin hard-denials; destructive/external can never auto-allow.
- **Model providers** (`nexus-models`): llama.cpp, Ollama, generic
  OpenAI-compatible, custom HTTP, and mock; task routing with fallback.
- **Sandbox** (`nexus-sandbox`): container, restricted-process, and mock
  backends, each reporting honest isolation. No model downloads.
- **Typed tools** (`nexus-tools`): filesystem, repo/git, terminal (+PTY),
  SSRF-guarded web, and diagnostics.
- **Durable goals** (`nexus-goals`): evidence-verified, crash-recoverable.
- **Guarded memory** (`nexus-memory`): secret-refusing, approval-gated, FTS5.
- **Context management** (`nexus-context`): bounded packing and safe compaction.
- **Code index** (`nexus-index`): heuristic symbol extraction for grounding.
- **Skills** (`nexus-skills`): versioned, payload-free, human-enabled.
- **MCP** (`nexus-mcp`): stdio client (untrusted-by-default) and curated
  read-only server.
- **CLI** (`snx`) and full-screen NEXUS TUI with no-color mode.
- Documentation, config schema, examples, and shell completions.

### Added (post-initial)

- **Interactive agent upgrade**: `/init`, `/title`, `/summary`, `/persona`,
  `/profile`, `/thinking`, `/branch`, `/commit`, and `/connector` now share one
  canonical command registry across CLI and TUI surfaces.
- **Durable continuity**: provider token/tool/runtime usage, exit timestamps,
  persona/profile selection, exact session approval grants, summary artifacts,
  and parent/child rollover links are stored through append-only migrations.
- **Personas, profile review, and RSI proposals**: project/global persona
  inheritance, explicit low-risk workflow learning, sensitive/conflicting
  trait review, improved bounded memory ranking, and disabled-by-default
  declarative skill proposals.
- **Local Git milestone**: status, diff, stage, unstage, restore, branches, log,
  and selected-file-only commits with diff preview and confirmation.
- **Connector catalog and custom endpoints**: Codex MCP/Agent Skill discovery
  imports disabled/untrusted without credentials; remote Ollama and
  OpenAI-compatible endpoints accept host/port or URL, TLS choices, connection
  tests, and model discovery.
- **Session handoffs**: `/summary` saves and copies a structured handoff, linked
  rollovers start with only the approved summary, and `/exit`/`/logout` restore
  the terminal before printing `snx resume <session-id>`.
- **Semantic themes**: `cyberpunk` and `edgerunner` palettes cover true-color,
  256-color, ANSI, and no-color terminals.
- **`openai` provider** for GPT: defaults `base_url` to `https://api.openai.com/v1`,
  requires an API key, uses native tool-calling.
- **Codex "Sign in with ChatGPT" auth** (`auth = "codex"`): reuse an OpenAI Codex
  CLI OAuth session (`~/.codex/auth.json`) instead of an API key. New `snx auth`
  command (`status`/`login`/`logout`); `login` offers device-code, API-key, and
  same-device browser flows via the trusted `codex` CLI. The reused token is
  redaction-registered and sent as `Authorization: Bearer` (+ `chatgpt-account-id`
  for OAuth sessions).

### Fixed (post-initial)

- Compatibility-mode planner/reviewer/researcher/documentation prose now ends a
  non-tool turn normally. Only explicit action JSON can invoke a tool; one
  concise schema correction is issued for malformed action payloads.
- Startup remains available with missing hosted credentials so `/connect` can
  repair them. Existing Codex CLI credentials require explicit consent, setup
  preserves hand-written configuration, and logout drops runtime secrets by
  terminating/reloading the active application context.
- Active goals and budgets attach to new sessions; staged-file restore,
  empty/unreadable instruction selection, atomic UI-state writes, effort
  persistence, session switching, tool counts, and destructive memory
  confirmation now follow their durable source of truth.
- Agent loop retry counter no longer reports an out-of-range attempt (e.g.
  `4/3`) before stopping at the retry budget.
- Codex Responses history replay now normalizes dotted harness tool names even
  when those tools are not exposed on the current turn, preventing
  `input[n].name` HTTP 400 failures. Deterministic provider 4xx errors now
  surface immediately instead of consuming the retry budget.
- TUI: honest header now shows workspace basename so model/agent/sandbox status
  stays visible; footer scroll hint corrected to `PgUp/PgDn`.

### Changed (post-initial)

- Replaced the SILENT-dominant startup banner with one canonical, responsive
  NEXUS lockup shared by boot, `/about`, `/version`, `/welcome`, provider login,
  CLI banners, and the installer.
- Startup now reveals icon, wordmark, attribution, and tagline in 360 ms, is
  immediately skippable, and falls back cleanly for reduced motion, CI,
  redirected output, limited color, short terminals, and ASCII-only terminals.

### Security

- SSRF, private-range, cloud-metadata, and DNS-rebinding protection in web tools.
- Secrets never forwarded to sandboxes or logs; memory refuses secret content.
- Web and MCP content treated as untrusted data, never instructions.
