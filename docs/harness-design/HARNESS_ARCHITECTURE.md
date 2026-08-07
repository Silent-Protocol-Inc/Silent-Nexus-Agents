# Silent Nexus Harness Architecture

**Status:** Governing · **Version:** 0.1 · **Owner:** Silent Protocol

This document owns the conceptual system design. Concrete crate APIs remain in
the implementation documentation and source code.

## 1. System boundary

```text
operator intent
      │
      ▼
CLI / TUI presentation
      │  shared command registry and typed actions
      ▼
control plane: session · goal · plan · task · route · context
      │
      ▼
model boundary: provider stream → typed proposal
      │
      ▼
enforcement plane: schema → role → workspace → policy → approval
      │
      ▼
execution plane: sandbox → limits → tool → output finalisation
      │
      ▼
evidence plane: redacted timeline · audit · diff · validation · artifacts
```

The model may propose data for the boundary, but it cannot call around the
boundary. The operator may approve an action, but the approval cannot widen the
compiled policy or sandbox beyond what the harness can represent.

## 2. Ownership model

| Concern | Owner | Invariant |
|---|---|---|
| Path and workspace confinement | `nexus-core` | canonical, symlink-safe, root-bounded paths |
| Risk and policy | `nexus-policy` | scopes narrow; dangerous actions never auto-allow |
| Provider routing and streaming | `nexus-models` | provider failure is explicit and recoverable |
| Isolation and limits | `nexus-sandbox` | every backend reports honest isolation |
| Tool schemas and action requests | `nexus-tools` | concrete arguments can escalate risk |
| Orchestration and continuation | `nexus-agent` | stages, budgets, retries, and evidence are durable |
| Timeline and audit | `nexus-core` / observability | append-only, redacted, stable identity |
| Presentation and approval UI | `nexus-tui` / CLI layer | view state never becomes authority |

No presentation module owns policy. No provider owns permission. No tool owns
its own approval. No generated summary replaces the event or artifact that
supports it.

## 3. Action lifecycle

```text
proposed
  → schema-valid
  → capability-eligible
  → workspace-valid
  → policy decision
  → approval pending (when required)
  → approved / denied
  → sandbox selected
  → executing
  → output redacted and sanitised
  → audited
  → validated / failed / interrupted
```

Each transition emits or updates a typed timeline event with stable session,
turn, trace, span, sequence, status, risk, and artifact references. A running
event may receive progress updates without changing its sequence or moving the
operator's viewport unexpectedly.

## 4. State model

The harness has separate state machines for:

- **session:** open, paused, continued, completed, failed;
- **goal:** proposed, active, blocked, completed, cancelled;
- **work:** direct, tracked, planned, promoted;
- **action:** proposed, waiting, approved, denied, running, completed, failed;
- **provider:** configured, available, limited, unavailable, authenticated;
- **sandbox:** selected, ready, degraded, refused;
- **presentation:** input, timeline, context, approval, modal command.

These states must not be collapsed into one generic “busy” flag. A provider can
be limited while a session is resumable; an action can be denied while the goal
remains active; the TUI can be in an approval modal while the timeline continues
to hold background events.

## 5. CLI/TUI contract

The command registry is the shared source of command identity, aliases, usage,
argument parsing, permissions, and result semantics. The CLI renders structured
results for scripts; the TUI renders the same result as a timeline card, panel,
menu, or modal. Slash commands are an input adapter, not a second command
system.

Every interactive command must have:

1. a discoverable name and help entry;
2. deterministic parse errors;
3. an explicit read/write or external-action classification;
4. a usable narrow-terminal behavior;
5. a JSON or structured equivalent where the command is scriptable;
6. a recovery or cancellation path if it can wait or mutate state.

## 6. Context and memory

Prompt construction is ordered and inspectable: immutable safety, provider
protocol, sandbox and policy, project instructions, approved profile/persona,
operational contract, goal and plan, guarded memory, session context, request.
The exact redacted manifest is persisted for the provider request. Compaction
removes prompt load, not history: original events remain durable and the summary
states what was folded and what was omitted.

## 7. Design-system inheritance

The website design system separates rules from values and composition from
identity. The harness adopts the same separation:

| Design-system idea | Harness equivalent |
|---|---|
| Genome | operator authority, visible boundaries, continuity, evidence |
| Constitution | safety floors, change process, verification duty |
| Tokens | typed command/status vocabulary, risk labels, time and size budgets |
| Components | approval, timeline event, command result, provider row, diff card |
| Patterns | plan approval, continuation, recovery, provider selection, validation |
| Archetypes | chat turn, one-shot run, status view, goal view, maintenance flow |
| Verification | adversarial tests, CLI/TUI parity, manual terminal review, evidence |

Values belong in typed configuration or code-owned constants; their meaning and
application belong in the owning contract. A timeline renderer must not invent
policy semantics, and a policy module must not prescribe panel layout. This is
the harness version of “one decision, one home.”

## 8. Change authority

Changes are classified before implementation:

- **presentation:** wording, layout, colour, key hints;
- **contract:** command names, flags, JSON shape, timeline fields;
- **control:** policy, approval, workspace, sandbox, redaction, credentials;
- **storage:** migrations, retention, serialization, recovery;
- **provider:** routing, authentication, fallback, capability claims.

Control, storage, and provider changes require adversarial tests and explicit
security/release review. Contract changes require CLI/TUI parity tests and
compatibility notes. Presentation changes must not obscure or weaken an existing
control.
