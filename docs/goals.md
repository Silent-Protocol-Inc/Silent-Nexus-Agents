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
