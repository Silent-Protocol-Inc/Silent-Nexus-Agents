# Memory and skills

## Long-term memory

Memory (`nexus-memory`) is durable, workspace-scoped by default, and guarded:

- **Refuses secrets.** If redaction would alter the content, the memory is
  rejected outright — secrets never enter the store.
- **Approval-gated.** Memories can require review before they influence future
  turns; only approved memories are retrieved into context. `snx memory approve
  <id>` promotes one.
- **Scoped.** `project` scope is the default; `global` (cross-workspace) memory
  is only permitted when `memory.global_enabled = true`.
- **Deduplicated and TTL'd.** Identical content in the same scope is collapsed;
  expired memories are removed by `snx memory prune`.

```sh
snx memory add "this project pins tokio without default features" --kind project_fact
snx memory search "tokio"
snx memory list --all
snx memory forget <id>
snx memory export        # JSON
```

Full-text search uses SQLite FTS5 with sanitized queries.

The agent writes memories through the `memory.add` tool, offered to every role
(recording a finding is not a workspace write, so a read-only role such as
`reviewer` may call it). Agent-authored entries are always candidates: they
appear in `/memory` and `snx memory list --all` straight away, and are retrieved
into later turns only after `snx memory approve <id>`. The number an agent may
write in one run is capped by `limits.max_memory_writes`.

Retrieval is bounded and re-ranked by scope, approval, confidence, correction
priority, verification/recency, and normalized-content deduplication. Memory is
still advisory: prompt precedence is immutable safety, policy/sandbox, project
instructions, selected persona, approved profile, retrieved memory, then the
session transcript.

## Personas and learned profiles

Personas are inspectable behavior cards with project/global scope, inheritance,
clone/edit/delete/select flows, and a maximum inheritance depth. Project
definitions override same-name global definitions. They cannot weaken safety,
policy, sandbox, provider, or project instructions.

Profiles store workflow traits with evidence, confidence, sensitivity, source
session, and review state. Explicit low-risk workflow preferences can be
approved automatically. Inferred, sensitive, identity-related, or conflicting
traits remain pending until accepted or rejected.

```sh
snx persona create focused "Prefer concise implementation evidence"
snx persona select focused
snx profile select work
snx profile add validation "run targeted tests before the full suite"
snx profile list --all
```

### Profile cards

A profile card is who SNX is talking to: a preferred name and a set of facts
with provenance, confidence, sensitivity, and review state. One card is active
per workspace, and a workspace that has never chosen one inherits your most
recent card rather than starting again as a stranger. An explicit choice is
never overwritten.

Cards fill in two ways, and both are visible in `/profile`.

**You say something durable.** A deterministic pre-turn pass reads named
wordings — `my name is …`, `call me …`, `I work as …`, `my timezone is …`,
`reply in …`, `I prefer …`, `I use …` — and records what they state. There is no
model call and no per-message classification: a wording that is not one of these
records nothing. Statements about right now (`I'm tired`), about other people
(`his name is …`), inside quotes or fences, or too long to be a fact are refused.

**The agent records it.** `profile.add_fact` lets an agent write down what you
told it in a wording the pass does not cover. It is documented as being for
what you *stated*, not what the agent inferred about you.

Either way: repeating yourself changes nothing rather than adding a duplicate,
and a changed value supersedes the previous one without discarding it — the old
fact stays on the card, marked superseded.

**What is never stored.** Passwords, API keys, tokens, private keys, auth
cookies, passphrases, payment-card numbers, and recovery codes are refused
outright and redacted from logs — the value is not held for review, it is not
held at all. Sensitive categories — health, religion, race or ethnicity,
political affiliation, sexuality, a precise home address, criminal history,
financial account data — are recorded as candidates and are *not in use* until
you approve them in `/profile`, which says so on the row.

Reading the card needs no permission; every role can. Writing it is gated on
the `profile.write` capability, which roles that work from external material —
the researcher and the read-only audit roles — do not hold, so nothing SNX finds
elsewhere can be filed as something you said.

## Skills

A skill (`nexus-skills`) is a **declarative workflow description**, never hidden
executable payload. Skills reference existing tools by name; they cannot
introduce new executables.

- Manifests are validated and **reject payload markers** (`#!/`, `<?php`,
  `eval(`, `base64,`, NUL bytes).
- **Agent-proposed skills are stored disabled** (`provenance = agent_proposed`)
  and are never auto-enabled or auto-granted permissions. A human must enable
  them, and enabling verifies every referenced tool is actually registered.
- Imported skills are always disabled on import.

```sh
snx skill list
snx skill show  <name>
snx skill import path/to/manifest.json   # stored disabled
snx skill enable <name>                  # explicit, verifies required tools
snx skill disable <name>
snx skill export <name>
```

After completed turns, deterministic RSI analysis can identify repeated
friction or repeated workflows. Capability changes are proposals, never silent
source edits: inspect them with `snx profile proposals`, then approve/reject.
Approved skill proposals are validated and stored disabled with
`provenance = agent_proposed`.
