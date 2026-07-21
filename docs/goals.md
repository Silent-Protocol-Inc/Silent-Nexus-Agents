# Goals

A goal is **durable workflow state**, not a prompt alias. Every field lives in
SQLite and survives restarts, and a goal is only `completed` when every
acceptance criterion has recorded, tool-sourced evidence.

## Lifecycle

```
draft → planned → running ⇄ waiting_approval ⇄ blocked ⇄ paused
                     ↓
                 verifying → completed
                     ↓
                  failed / cancelled   (terminal)
```

Transitions are validated (`GoalStatus::can_transition_to`) and journaled to the
`goal_events` table. A transition to `completed` is refused unless verification
passes.

## Evidence-based verification

Each acceptance criterion is satisfied only by an `EvidenceItem` that names the
criterion index, the source tool that produced it (`repo.check`, `terminal.run`,
`fs.read_file`, …), whether it passed, and optionally an artifact holding the
full proof. `snx goal verify <id>` reports which criteria still lack evidence.
The model's assertion that a goal is done carries no weight on its own.

## Plan mode

`/plan` with no arguments enters plan mode. While it is on:

- The turn runs under a policy scope named `plan-mode` that allows only reading
  tools (`fs.read_file`, `fs.list_dir`, `fs.find_files`, `fs.search_text`,
  `repo.git_*`, `diag.*`) plus `plan.submit`. Writes, commands, network, and
  delegation are **refused by the engine**, not by the model's restraint. The
  allowlist is deliberately positive, so a tool added later is denied until
  someone decides otherwise. `repo.check` is excluded because it runs builds and
  tests.
- Your `/permissions` preset is untouched. The scope lives on the per-turn
  policy engine and dies with the process, so a crash while planning cannot
  strand the workspace in read-only.
- The agent reads the workspace and then calls `plan.submit` once, with ordered
  steps that name the files each one touches and how it is verified. That
  becomes a real `WorkBreakdown` — not the
  Grounding / Implementation / Validation template `/plan create` still uses.
- You approve or decline it. **Approving ends the mode and runs the plan in the
  same turn**, with the full tool surface restored. Declining keeps the draft
  and the mode, so your next message refines the plan instead of starting over.
- Asking a question while planning is fine: a turn that submits nothing is just
  a read-only answer.

`/plan exit` (or `/plan cancel`) leaves without approving; any draft remains
stored and shows up in `/plan history`. Every other subcommand — `create`,
`edit`, `approve`, `run`, `pause`, `resume`, `replan`, `verify`, `history`,
`export` — is unchanged.

## Budgets and recovery

Goals carry a step budget and a runtime budget; both are consumed as work
progresses and enforced by the loop. Goals interrupted mid-run (status
`running`/`verifying` at restart) are listed by `snx goal recover` so work can
resume deterministically.

## CLI

```sh
snx goal new "Ship the parser" -c "cargo test passes" -c "docs updated"
snx goal list
snx goal show  <id>
snx goal verify <id>
snx goal recover
snx goal export <id>     # full JSON, for audit or handoff
```
