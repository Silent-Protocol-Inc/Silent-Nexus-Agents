# Changelog

All notable changes to NEXUS are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project adheres to
Semantic Versioning.

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
