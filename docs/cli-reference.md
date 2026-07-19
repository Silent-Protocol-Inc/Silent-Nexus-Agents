# CLI reference

Run `snx help`, `snx help <command>`, or `man snx` for generated argument
details. Global flags include `--json`, `--no-color`, `--inline`, and
`--verbose`.

## Work and continuity

- `snx` / `snx chat`: interactive TUI.
- `snx run <objective> [--agent ROLE] [--session ID] [--yes]`: one objective.
- `snx status`, `snx usage`: active harness/provider status.
- `snx resume [ID]`, `continue [ID]`, `summary [--session ID]`: durable
  continuation and handoff.
- `snx test [COMMAND...]`: approved configured or explicit validation command.

## Goals, sessions, and customization

- `snx goal ...`: create, list, inspect, pause, resume, cancel, verify, and
  export evidence-backed goals.
- `snx session ...`: list, inspect, and title durable sessions.
- `snx persona ...`, `profile ...`, `theme [NAME]`: operator-controlled behavior
  and presentation.
- `snx memory ...`, `skill ...`: guarded memory and declarative skills.

## Models and integrations

- `snx setup [--project] [--force]`: detect local runtimes and write a starter.
- `snx model ...`: select and test a model through the provider-first picker.
- `snx catalog list|health [--json]`: read-only provider-grouped inventory,
  capabilities, freshness, availability, routing defaults, and health.
- `snx auth ...`: consent-gated provider authentication.
- `snx connector ...`: inspect/import connector definitions disabled/untrusted.
- `snx mcp ...`: MCP registry/client and curated read-only server.

## Repository and tools

- `snx init`: inspect project instructions and optionally create `AGENTS.md`.
- `snx branch ...`: local status, diff, stage, restore, log, and branch actions.
- `snx commit -m MESSAGE -f PATH... [--allow-hooks]`: preview and commit only
  selected files. Hooks are disabled unless explicitly opted in.
- `snx index ...`, `tools ...`: code index and tool registry.
- `snx sandbox status|test`: actual isolation and diagnostics.

Generic terminal Git commit, push, remote, aliases, and unrecognized operations
are denied; use the typed repository commands.

## Diagnostics and maintenance

- `snx about`: identity and embedded version/target/profile/commit/epoch.
- `snx doctor [--deep]`: readiness; deep mode adds state, release, isolation,
  permissions, and binary-integrity checks.
- `snx maintenance check`: non-destructive integrity report.
- `snx maintenance backup <directory>`: create a new atomic snapshot directory.
- `snx maintenance optimize [--vacuum]`: optimize/checkpoint, refusing active
  foreground/background work.
- `snx audit [--kind KIND] [--limit N]`, `logs`: audit/log locations.
- `snx config show|path|schema`: redacted effective config and schema. The TUI
  `/config` hub writes typed workspace/global managed overrides and supports
  explicit inheritance/reset; credentials remain in `/login` and `/connect`.
- `snx completion <shell>`: generate shell completions.

Maintenance commands never prune transcripts, tasks, plans, goals, memories, or
artifacts.
