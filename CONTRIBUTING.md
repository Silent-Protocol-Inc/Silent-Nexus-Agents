# Contributing

Silent Nexus is a safety-critical harness. Contributions are welcome, but the
bar is: **the model must never be able to bypass a safety boundary.**

## Ground rules

- **No placeholders.** No fake handlers, simulated sandboxes, hardcoded model
  replies, or tests that only assert `true`. If a capability is advertised, it
  must be real; if a limitation exists, state it honestly in code and docs.
- **Safety lives in the harness.** Any new capability must route through schema
  validation, the workspace guard, the policy engine, the sandbox, redaction,
  and audit — never around them.
- **Honest reporting.** Never claim stronger isolation, health, or success than
  is actually true.

## Development

```sh
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo build --release
```

All four must pass. The workspace denies clippy warnings (including
`unwrap_used` outside of justified cases) and requires formatted code.

## Adding a tool

1. Implement the `Tool` trait in `nexus-tools`. Declare an accurate `ToolMeta`
   (risk, category, JSON Schema, side-effects).
2. Build a precise `ActionRequest` in `action_request`, escalating risk on the
   concrete arguments (e.g. recursive delete → `Destructive`).
3. Add adversarial tests: malformed args, path-escape attempts, oversized
   output, and — for command/web tools — injection and SSRF cases.

## Adding a provider

Implement `ModelProvider` in `nexus-models` and wire it into
`ModelManager::build_provider`. Support the no-native-tool-calls path.

## Commit hygiene

Keep changes focused. Update the relevant `docs/*.md` when behavior changes, and
add or extend tests for every behavioral change.
