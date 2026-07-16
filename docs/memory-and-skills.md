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
