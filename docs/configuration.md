# Configuration reference

Silent Nexus configuration schema version is `1`. Unknown fields are rejected,
and `snx config schema` prints the machine-readable JSON Schema embedded in the
installed binary.

## Precedence

Values merge from lowest to highest precedence:

1. secure built-in defaults;
2. user config (`~/.config/silent-nexus/config.toml` on Linux, subject to
   `XDG_CONFIG_HOME`);
3. machine-managed model definitions (`models.toml`);
4. workspace config (`<workspace>/.nexus/config.toml`);
5. machine-managed global interactive overrides (`overrides.toml`);
6. machine-managed workspace overrides (`<workspace>/.nexus/overrides.toml`);
7. supported `SNX_*` environment overrides;
8. explicit command flags.

Interactive commands update managed files and do not replace hand-written
configuration. Files under the user config root are private (`0700` directories,
`0600` files).

## General and routing

`[general]` controls `theme`, `no_color`, `reduced_motion`,
`default_agent`, and the optional `test_command`.

Fresh or omitted configuration defaults `default_agent` to `nexus`, the
general-purpose role. Explicit existing values remain unchanged. The role has
no specialist prompt, task-class restriction, output contract, or reduced tool
category, but every normal policy, approval, denial, sandbox, Full Access,
redaction, and audit boundary remains mandatory.

`[routing]` maps `simple`, `coding`, `planning`, and `fallback` to names defined
under `[models.<name>]`. A route to a missing model is rejected.

## Models

Each model entry accepts:

- `provider`: `llamacpp`, `ollama`, `openai`, `openai_compatible`,
  `custom_http`, `codex`, `claude-plan`, `anthropic`, or `mock`;
- `base_url` and `model`;
- `api_key_env` or `api_key_ref` (never an inline secret);
- optional `auth = "codex"`;
- `context_window`, `max_output_tokens`, `limit_mode`, cached limit provenance,
  `role`, `native_tool_calls`,
  `temperature`, `reasoning_effort`, `timeout_secs`, and `tls_verify`.

Hosted credentials are resolved into zeroized secret buffers and registered
with the redactor. Provider auth profiles live under the private auth root.

## Policy

`[policy]` fields `reads`, `writes`, `commands`, `network`, and `downloads`
accept `allow`, `ask`, or `deny`. `destructive` and `external` accept only
`ask` or `deny`.

`denied_commands`, `allowed_commands`, and `denied_paths` narrow behavior.
Allowlisted commands apply only to proved structured argv. They never cover raw
shell, interpreters, wrappers, unrecognized commands, destructive actions, or
approval-only host execution.

Built-in denials cannot be overridden: privilege escalation and generic
terminal Git commit/push/remote/alias/unrecognized operations remain denied.

`[policy.read_formats]` maps stable format ids such as `rust`, `toml`,
`silent`, and `other` to `allow`, `ask`, or `deny`. Workspace keys override
global defaults. Sensitive environment files, credentials, private keys,
`.git`, and `.nexus` are locked denials. Full Access overrides ordinary format
rules only for the current attended session and never overrides locked paths.

## Sandbox

`[sandbox]` fields:

- `backend`: `auto`, `container`, `process`, or `none`;
- `container_image`: mandatory digest-pinned OCI reference;
- `cpu_limit_secs`, `memory_limit_mb`, `timeout_secs`;
- `max_output_bytes`: shared stdout/stderr kill budget;
- `network`: `off`, `restricted`, or `full`;
- `env_allowlist`.

The default image is:

```text
debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818
```

`auto` selects the container only when its engine and pinned image are already
available locally; Silent Nexus does not pull it. Otherwise it selects the
approval-only host guardrail. Automatic/background terminal execution is
permitted only with strong container isolation.

### Restricted files and host commands

Restricted paths are masked by bind-mounting `/dev/null` over them, so **only
the container backend can mask anything**. Without a container, a host command
inherits your own read access, and nothing can stop it reading `.git`, `.env`,
or a keystore — so terminal actions are refused instead, after approval, with a
count of the restricted paths.

Two things already count as the operator having decided, and neither needs the
setting below:

- **approving the action.** A host terminal action raises a prominent one-time
  prompt that states the action is not isolated. Answering it runs that one
  action; the next one asks again.
- **full access.** `commands = "allow"` is the same answer given once instead of
  per action, so a structured `program + argv` invocation runs and is audited
  rather than prompting. Raw shell (`terminal.run`) still asks every time — an
  arbitrary command line is worth reading before it runs, whatever the mode.
  Full access applies only while someone is there to have chosen it: unattended
  and background runs still cannot execute on a host backend.

Otherwise `.git` is restricted, which means the refusal applies in **every Git
repository**, not only unusual workspaces. Three ways forward:

- use `fs.read_file` and `fs.search`, which are checked per file and are
  unaffected;
- enable the container sandbox (`/sandbox`), which masks the paths for real;
- set `[sandbox].allow_unmasked_host_reads = true` to run host commands anyway.

The last one is a real widening, not a formality: it lets a host command read
paths that `fs.read_file` refuses individually. Every action is still approved
one at a time, and the approval card states that the action is not isolated. Set
it per workspace with:

```sh
snx config set sandbox.allow_unmasked_host_reads true --workspace
```

## Web, memory, limits, and MCP

`[web]` controls enablement, search provider, fetch size, host allow/deny lists,
loopback access, per-host delay, and timeout. SSRF and redirect validation still
apply when web access is enabled.

`[memory]` controls persistent and cross-workspace memory plus default TTL.
Secret-like content is refused.

`[limits]` bounds turn steps, retries, repeated calls, goal steps/runtime, and
reserved completion tokens. `/config budgets` edits the whole block
interactively; `snx config budgets` prints the effective values.

`max_tokens_per_turn` exists to bound spend, so it does not apply to a server
you run yourself: a turn routed to `ollama` or `llamacpp` uses
`self_hosted_max_tokens_per_turn` (default `5000000`) instead. A turn that falls
back onto a metered provider is re-bounded by the metered ceiling immediately.

`self_hosted_context_window` (default `32768`) is the context an `ollama` or
`llamacpp` model is auto-configured with when its provider reports only an
architecture maximum. It is one number for every such model; see
[providers.md](providers.md#changing-the-window) for how to raise it or pin a
single model instead.

The budget a turn spends is **weighted**, not a raw token sum: a cache read
counts at a tenth and a cache write at five-quarters, so a warm turn survives
where an identical cold one would pause. Three nested blocks tune the guard, all
with `#[serde(default)]` so an existing config is untouched:

```toml
[limits.local_runaway_guard]
enabled = true
max_weighted_tokens = 250000    # inherits max_tokens_per_turn when unset
max_no_progress_cycles = 3      # cycles without progress before recovery + pause
max_identical_tool_repeats = 3  # identical calls that trip the guard at once

[limits.context_compaction]
enabled = true
trigger_ratio = 0.75            # fraction of the window that triggers compaction

[limits.retry]
max_attempts = 3
max_wait_seconds = 120          # a 429 whose reset exceeds this pauses instead
```

`max_tokens_per_turn` and `self_hosted_max_tokens_per_turn` are kept and honored
as the guard's ceiling when `max_weighted_tokens` is unset — no old numeric value
is reinterpreted. When the guard is reached with progress still being made, the
turn compacts its history and continues rather than stopping; it pauses resumably
only when nothing is left to compact or progress has stalled.

`[profile]` controls what SNX records about you from what you say.
`auto_capture` (default `true`) enables the deterministic pre-turn pass;
`capture_preferences` (default `true`) additionally captures stated working
preferences, not only identity; `require_review_for_sensitive` (default `true`)
holds sensitive categories as candidates you approve in `/profile` rather than
putting them into use. Turning `auto_capture` off does not disable profiles —
the agent's `profile.*` tools still work, so facts are recorded only when you
ask for them. See [memory-and-skills.md](memory-and-skills.md#profile-cards).

`[tui.activity]` controls the live activity segments shown during a turn.
`tool_icons` selects the glyph tier — `"geometric"` (default), `"emoji"`, or
`"ascii"`; `SNX_ASCII`, `TERM=dumb`, and a `C`/`POSIX` locale force ASCII
regardless. Activity text is harness-derived and never carries private reasoning.

`[mcp.<name>]` configures `stdio` or `http`, command/arguments or URL, enabled
state, trust, environment allowlist, and timeout. Imports are disabled and
untrusted until explicitly reviewed.

See [`../examples/config.toml`](../examples/config.toml) for a complete example.
