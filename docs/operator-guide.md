# Operator guide

## First start

1. Install the verified binary and run `snx --version`.
2. Run `snx setup`, or create a workspace/user config that references a model.
3. Pull the exact digest-pinned container image if automatic terminal execution
   is required.
4. Run `snx doctor --deep` and `snx sandbox status`.
5. Start with read-only/research work before granting write capabilities.

Silent Nexus does not download models or container images automatically.

## Reading approvals

An approval identifies the tool, concrete paths/argv, risk, policy reason, and
actual isolation. Verify all of them. Raw shell, interpreters, wrappers,
unproved commands, and host-process execution are one-time only. Session grants
are limited to proved non-destructive argv under strong isolation.

The process backend is not containment. If an action needs a host-process
approval, assume it can see everything available to your user account despite
path validation, environment scrubbing, resource limits, and process-group
cleanup.

## Planning before acting

`/plan` enters plan mode: the agent may read the workspace and nothing else,
and its job for that turn is to hand you a plan to approve. The status bar shows
`PLAN` and the input frame says so. Approving the plan ends the mode and runs it
in the same turn; `/plan exit` leaves without approving. See
[goals.md](goals.md#plan-mode) for what the mode allows and refuses.

Plan mode is not a substitute for approvals. It narrows what a turn *can*
propose; the approvals above still gate everything the turn does afterwards.

## Containers

The strong backend runs as the invoking UID/GID, mounts the workspace read-only
for read actions and writable only for approved writes, hides `.git`, `.nexus`,
and detected credentials, disables network unless approved, drops
capabilities, and applies memory/CPU/PID/output/time limits.

Keep the image digest pinned. Review image changes as supply-chain changes.

## Routine checks

Before and after significant work:

```sh
snx status
snx audit --limit 50
snx maintenance check
```

Before an upgrade or risky operation:

```sh
snx maintenance backup "$HOME/snx-backup-$(date +%Y%m%d-%H%M%S)"
```

Use `snx maintenance optimize` periodically for long-lived active workspaces.
Use `--vacuum` only when you explicitly want a blocking database rewrite and no
work is active.

## Incident response

Stop the foreground turn, preserve `.nexus/state` and logs, avoid vacuuming or
editing the database, and run `snx doctor --deep` plus `snx maintenance check`.
If artifact or migration integrity fails, restore from a known-good backup
instead of bypassing validation.
