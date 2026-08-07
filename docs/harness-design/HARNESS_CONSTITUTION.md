# Silent Nexus Harness Constitution

**Status:** Governing · **Version:** 0.1 · **Owner:** Silent Protocol

This Constitution governs how the CLI harness is designed, changed, verified,
and operated. It is subordinate only to explicit security and release policy.
The Genome says what the harness is; this document says how we are allowed to
change it.

## I. Purpose

The harness exists to make model-assisted work useful without making the model
the authority. The operator owns intent and consent. Rust-owned boundaries own
capability, policy, execution, persistence, and evidence.

## II. Absolute laws

### 2.1 The model is an untrusted planner

Model text, tool arguments, retrieved content, MCP results, repository content,
and command output are untrusted input. No model claim is a permission, a fact,
or release evidence by itself.

### 2.2 No bypass around the control plane

Every world-affecting action passes, in order, through schema validation,
capability and role checks, workspace confinement, policy evaluation, approval
when required, sandbox selection, limits, redaction, terminal sanitisation, and
audit recording. A new path that skips one of these is invalid by construction.

### 2.3 Consent must describe the real action

An approval shows the concrete tool, paths or argv, risk, policy reason, and
actual isolation. “Allow” never means “allow whatever the model does next.”
Session grants are narrower than one-off consent and are unavailable to raw
shell, interpreters, wrappers, destructive actions, unproved commands, or
approval-only host execution.

### 2.4 Fail closed at uncertainty boundaries

Missing isolation, ambiguous command structure, invalid arguments, unavailable
credentials, unknown paths, stale approval scope, and incomplete evidence stop
the action or require a new attended decision. The harness never silently
upgrades uncertainty into access.

### 2.5 Secrets do not become interface content

Credentials use protected types and private storage. Redaction happens before
model context, transcript, audit output, diagnostics, or terminal display. A
secret appearing in a log or fixture is a defect, not a test convenience.

### 2.6 Durable state is append-only in meaning

Timeline, audit, goal, task, transcript, memory, and artifact history may gain
compatible fields but are not silently rewritten or automatically deleted.
Migration history is append-only. Recovery must preserve what happened, not only
the latest status.

### 2.7 Evidence outranks narration

The harness distinguishes proposed, approved, attempted, completed, validated,
and independently verified. A green-looking status, model summary, or final
answer cannot stand in for command output, test results, diff evidence, or an
explicit unknown.

## III. Operator floors

- The current workspace, branch, provider/model, sandbox, policy, goal, and
  pending approvals are visible when they affect the next action.
- Approval choices are explicit: approve once, approve for the allowed scope,
  choose a safer alternative when offered, or deny.
- The same command registry and behavior powers non-interactive CLI and TUI
  commands. Presentation may differ; authority may not.
- `--json` is stable machine output, not a pretty rendering of terminal text.
- Interrupt, denial, provider failure, timeout, and restart are normal states
  with recovery paths, not exceptional blank screens.
- Raw provider reasoning is not persisted or displayed as if it were an audit
  record. Harness-derived activity and safe summaries are explicit and bounded.

## IV. Decision procedure

The following design-office principles are inherited from the website design
system and adapted for an operational CLI:

### 4.1 Make an argument before choosing a treatment

Every command, panel, status card, and prompt must be able to state what it is
claiming about the operator's task. A progress card should explain progress; an
approval card should explain authority and risk. A visual treatment is not a
reason.

### 4.2 Structure before decoration

Information hierarchy, state transitions, keyboard reachability, and recovery
come before colour, glyphs, animation, or personality. A terminal theme may
change; the action boundary and evidence order may not.

### 4.3 One decision, one authoritative home

Policy values, command semantics, timeline meaning, and presentation rules each
have one owner. Other documents and surfaces reference that owner. Duplicating a
rule in the CLI, TUI, and operator guide is a drift risk, not consistency.

### 4.4 Measure what can be measured

Record command duration, output limits, token and cost usage, retry counts,
approval scope, isolation strength, contrast where relevant, and validation
results when those properties are part of the claim. “Fast,” “safe,” and
“complete” are not evidence without a measurement or an explicit test status.

### 4.5 Design for the worst realistic condition

The design must hold at a narrow terminal width, with long paths, wrapped
errors, no colour, reduced motion, a missing provider, a denied action, a full
timeline, a slow model, and a user returning after interruption. The happy path
does not define the product.

### 4.6 Try deletion before addition

Before adding a badge, panel, confirmation, status line, shortcut, or animation,
remove an existing element and test whether the operator loses a real decision
or piece of evidence. If not, delete rather than accumulate.

Before changing a behavior:

1. State the operator problem and the security or continuity risk.
2. Identify the owning layer and the existing invariant being preserved.
3. Prefer a typed, reversible change with the smallest authority surface.
4. Add adversarial tests for any changed boundary.
5. Record the evidence and the remaining limitation.

When uncertain, choose the reversible decision. When the evidence is conclusive,
choose the correct decision even if it is less familiar or less convenient.

If the same exception appears three times, it is a missing rule and must be
resolved at the owning architectural layer rather than copied again.

## V. Verification floor

Security-sensitive or release-facing changes require the repository's pinned
toolchain checks: formatting, locked tests, locked Clippy, rustdoc warnings,
security scans, and release validation as applicable. Documentation must say
whether an assertion is tested, manually inspected, measured, or not yet
possible because the infrastructure does not exist.
