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
5. machine-managed interactive overrides (`overrides.toml`);
6. supported `SNX_*` environment overrides;
7. explicit command flags.

Interactive commands update managed files and do not replace hand-written
configuration. Files under the user config root are private (`0700` directories,
`0600` files).

## General and routing

`[general]` controls `theme`, `no_color`, `reduced_motion`,
`default_agent`, and the optional `test_command`.

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

## Web, memory, limits, and MCP

`[web]` controls enablement, search provider, fetch size, host allow/deny lists,
loopback access, per-host delay, and timeout. SSRF and redirect validation still
apply when web access is enabled.

`[memory]` controls persistent and cross-workspace memory plus default TTL.
Secret-like content is refused.

`[limits]` bounds turn steps, retries, repeated calls, goal steps/runtime, and
reserved completion tokens.

`[mcp.<name>]` configures `stdio` or `http`, command/arguments or URL, enabled
state, trust, environment allowlist, and timeout. Imports are disabled and
untrusted until explicitly reviewed.

See [`../examples/config.toml`](../examples/config.toml) for a complete example.
