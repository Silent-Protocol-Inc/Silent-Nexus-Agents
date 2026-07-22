# Architecture

Silent Nexus treats the language model as an **untrusted planner**. Every
capability that can affect the world lives in Rust, on the harness side of a
boundary the model cannot cross. This document describes the per-turn pipeline
and the crate responsibilities that enforce it.

The 1.1 adaptive control-plane architecture and its validation status are
tracked in [`adaptive-harness-delivery-report.md`](adaptive-harness-delivery-report.md).
That report is an evidence template, not a claim that an unvalidated release is
complete.

## The controlled agent loop

Each turn runs this pipeline (`nexus-agent`):

```
objective
  → classify (deterministic keyword classifier → TaskClass)
  → route model (task class → configured model, with fallback)
  → attach durable session goal/budgets/persona/profile
  → classify Direct / Tracked / Planned work and persist its stages
  → load enforced policy and sandbox scope
  → select agent role + minimal tool subset (role ∩ task class)
  → build prompt (immutable safety → provider policy → sandbox/policy
                  → project instructions → agent → persona → approved profile
                  → memory → approved plan/tasks → session context)
  → persist the redacted ContextManifest for the exact provider request
  → stream model text/tool deltas into stable timeline cards
  → parse action (native call, strict compat JSON action, or terminal prose)
  → validate against the tool's JSON Schema
  → evaluate policy → allow / allow_session / ask / deny
  → request approval if required (CLI prompt / TUI modal)
  → execute in the sandbox (timeouts, rlimits, output caps)
  → capture output → redact secrets → sanitize terminal control codes
  → audit the tool call
  → update stage evidence, changed files, and independent validation
  → persist token/tool/runtime usage and goal consumption
  → continue / recover (bounded retries) / finish
```

The crucial property: **no arrow in that chain can be skipped by the model.**
Schema validation, capability checks, path restrictions, approval policy,
sandboxing, timeouts, output limits, redaction, and audit logging are all
invoked by the harness regardless of what the model emits.

## Trust boundary

```
        ┌─────────────────────────── UNTRUSTED ───────────────────────────┐
        │  model output · tool arguments · web content · MCP tool results  │
        └───────────────────────────────┬──────────────────────────────────┘
                                         │  must pass through, in order:
   ┌─────────────────────────────────────▼─────────────────────────────────┐
   │ 1 JSON Schema validation        (nexus-tools::validate_args)           │
   │ 2 capability / role gate        (AgentRole::tool_categories)           │
   │ 3 workspace confinement         (nexus-core::WorkspaceGuard)           │
   │ 4 policy evaluation             (nexus-policy::PolicyEngine)           │
   │ 5 human approval (if required)  (ApprovalHandler)                      │
   │ 6 sandboxed execution           (nexus-sandbox::SandboxManager)        │
   │ 7 output cap + timeout          (backend enforced)                     │
   │ 8 secret redaction              (nexus-core::Redactor)                 │
   │ 9 terminal sanitization         (nexus-core::sanitize)                 │
   │10 audit record                  (nexus-observability::AuditLog)        │
   └────────────────────────────────────────────────────────────────────────┘
                                         │
        ┌────────────────────────────────▼─────────────────────────── TRUSTED ┐
        │  filesystem · processes · network · persisted state                  │
        └──────────────────────────────────────────────────────────────────────┘
```

## Crate responsibilities

- **nexus-core** — the invariants everything else depends on: `WorkspaceGuard`
  (canonicalizes every path and rejects anything outside the workspace root or
  matching the denied set, with symlink-swap protection on writes), `Redactor`
  (secret patterns + registered env values), `sanitize` (strips CSI/OSC/DCS
  control sequences before any text reaches a terminal), `RiskLevel`/`Decision`,
  layered `Config`, the SQLite `Store` (WAL, 0600 perms), typed IDs, audit
  event types, the append-only execution timeline, context manifests, durable
  plans/tasks/agent runs, interruptions, and content-addressed artifacts.
- **nexus-policy** — evaluates a normalized `ActionRequest` against builtin
  hard-denials (e.g. `sudo`), policy scopes (which may only *narrow*), an
  allowlist, exact normalized-command session grants, and per-category defaults. Destructive and
  external actions can never be configured to `allow` — at most `ask`.
- **nexus-models** — streaming providers for local runtimes, OpenAI-compatible
  APIs, Codex, the consent-gated Claude CLI plan bridge, and native Anthropic.
  `ModelManager` routes by task class and the compatibility layer lets models
  without native tool-calling participate through a strict textual protocol.
- **nexus-sandbox** — `SandboxManager::select` picks a backend and records
  honest `selection_notes`; each backend reports an `IsolationReport` describing
  exactly what it does and does not isolate.
- **nexus-tools** — every tool declares a `ToolMeta` (risk, category, schema,
  side-effects) and builds its own `ActionRequest` so risk can escalate on the
  concrete arguments. `finalize_output` redacts, sanitizes, truncates, and
  spills overflow to an artifact.
- **nexus-agent** — the loop above, complexity-aware promotion, plan/stage
  progress, custom agents that may only narrow an audited base role, durable
  usage/continuation records, and bounded subagent orchestration. Provider
  reasoning summaries, tool plans, approvals, execution, diffs, validation,
  retries, and final answers are separate events.
- **nexus-goals / nexus-memory / nexus-context / nexus-index / nexus-skills /
  nexus-mcp** — durable goals, guarded memory, safe compaction, code
  intelligence, payload-free skills, and MCP client/server.

## Long sessions

A session is not capped by the model's context window. When stored history
would take more than 75% of the prompt budget, the loop folds the older
messages into the session summary *before* the prompt is compiled:

- The summary is written by the routed model — a real recap of what was asked,
  what was decided, which files and commands were touched, and what is
  unresolved. If that call fails the turn still proceeds with a mechanical
  outline, and the timeline card and toast say so rather than implying an
  equivalent summary.
- Three things are never folded: system messages, the session's first user
  message (the objective), and the six most recent messages.
- Folded rows are marked `compacted` and stay on disk. `messages()` stops
  returning them, so the model never sees a message and its summary at once,
  but the transcript and audit trail are unchanged.
- The result is persisted, so a span is summarized once instead of being
  re-derived every turn, and repeated compactions append rather than overwrite.

`ContextCompiler` still trims sections to fit as a last resort; it should now
rarely have to, because the durable fold runs first and at a lower threshold.
`/compact` remains the manual path and is unchanged: it starts a fresh session
and leaves the original intact.

## Timeline and active work

Migration `0004_orchestration_timeline` adds append-only timeline,
context-manifest, view-state, plan, task, agent-run, and interruption tables.
Existing messages/tool calls/approvals/audit rows remain valid and are projected
into typed cards when a session has no native timeline events.

Every native event has stable session/turn/trace/span identity, sequence,
timestamp, lifecycle phase, status, summary, redacted payload, duration, risk,
and artifact references. Streaming updates mutate one running card without
changing its sequence, so viewport anchors and exports remain stable.

The TUI initially loads the newest page, requests older pages while scrolling
up, measures wrapped rows, preserves the viewport when history is prepended or
the terminal resizes, and follows new activity only while the operator remains
at the bottom. `--inline` keeps the same event model while using native terminal
scrollback instead of the alternate screen.

## Plans, tasks, and continuation

Direct work remains a single active stage. Tracked work gets a short durable
checklist. Planned work permits read-only grounding but requires a visible plan
approval before its first write/external action. If observed actions expand,
the loop promotes the work breakdown and records the versioned promotion.

The on-demand worker leases at most three readers and one writer. Writer tasks
use persistent `snx/task/<id>` branches in sibling Git worktrees outside the
source checkout. NEXUS never auto-commits, merges, stashes, or deletes them.

`/continue` writes a redacted checkpoint artifact and creates a linked child
session. The child receives a cloned plan with the same stage/evidence state,
and write idempotency is scoped to the rollover root so completed parent writes
cannot be repeated under a new child session id. Provider-limited children stay
paused until the operator selects a usable model.

See [`threat-model.md`](threat-model.md), [`sandbox-security.md`](sandbox-security.md),
[`policy.md`](policy.md), and [`goals.md`](goals.md) for subsystem detail.
