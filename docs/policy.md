# Policy and approval

The policy engine (`nexus-policy`) decides, for every proposed action, one of:

- **allow** — proceed without asking;
- **allow_session** — proceed and remember this exact action for the session;
- **ask** — require human approval;
- **deny** — refuse.

## Layers (evaluated in order)

1. **Builtin hard-denials.** Certain programs (e.g. `sudo`) and privileged-risk
   actions are denied unconditionally and cannot be re-enabled by configuration.
2. **Policy scopes** (global → user → project → agent → goal → session). A scope
   may only make policy *stricter* — deny tools, restrict paths. It can never
   grant what a broader layer denies.
3. **Allowlist.** Exact command prefixes the operator has pre-approved.
4. **Session grants.** The exact normalized argv of a command the operator
   approved "for the session" earlier. Grants are stored against that session,
   survive later turns/resume, do not widen to other arguments for the same
   program, and never cover destructive-or-higher risk.
5. **Category defaults.** Per-category decisions from config: `reads`, `writes`,
   `commands`, `network`, `downloads`, `destructive`, `external`.

## Invariants

- `destructive` and `external` may be configured to `ask` or `deny` — **never**
  `allow`. Config validation rejects `allow` for these.
- Risk can escalate on concrete arguments: a tool with base risk `write` becomes
  `destructive` when, for example, asked to delete a directory recursively.
- Shell metacharacters and known-dangerous programs are detected before any
  command runs; the raw-shell path is only taken for an explicitly approved
  command line.

## Configuration

```toml
[policy]
reads       = "allow"   # allow | ask | deny
writes      = "ask"
commands    = "ask"
network     = "ask"
downloads   = "ask"
destructive = "ask"     # may not be "allow"
external    = "ask"     # may not be "allow"
denied_commands = ["sudo", "rm -rf /"]
allowed_commands = ["cargo check", "cargo test"]
denied_paths = [".ssh", ".aws/credentials"]
```

Defaults are conservative: reads allowed, everything with side effects asks.
