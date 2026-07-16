# Security policy

Silent Nexus is a security-sensitive tool: it executes actions proposed by a
language model on your machine. Its entire design assumes the model may be
adversarial. Read this before granting write or command capabilities.

## Design principles

1. **Security over autonomy.** When safety and capability conflict, safety wins.
2. **Deterministic validation over model confidence.** The harness verifies;
   the model asserts nothing that isn't checked.
3. **Narrow tools over shell.** Prefer typed, schema-validated tools to raw
   command execution.
4. **Explicit approval.** Side-effecting actions ask unless you configured them
   otherwise; destructive/external actions can never be auto-allowed.
5. **Local-first.** No data leaves your machine unless a tool you approved sends
   it. No telemetry.
6. **Honest limitations over fake capabilities.** Isolation levels, health, and
   failures are reported truthfully.

## What is enforced in the harness (not the model)

Schema validation, capability/role gating, workspace confinement, layered
policy + approval, sandboxed execution, timeouts and output caps, secret
redaction, terminal sanitization, SSRF/DNS-rebinding protection, and audit
logging. See [`docs/threat-model.md`](docs/threat-model.md).

## Operator responsibilities

- **Choose a real sandbox for untrusted code.** The restricted-process backend
  is a guardrail, not containment. Use the container backend for hostile or
  model-generated code. Check `snx sandbox status`.
- **Keep secrets in environment variables**, referenced by `api_key_env`.
  Never inline keys in config.
- **Read approval prompts.** They state the tool, risk, exact command/paths, and
  whether the action is being isolated.
- **Review the audit log** (`snx audit`) after unattended runs.

## Reporting a vulnerability

Report suspected vulnerabilities privately to the maintainers rather than via a
public issue. Include a reproduction and the affected version (`snx --version`).
Please do not open public issues for undisclosed vulnerabilities.
