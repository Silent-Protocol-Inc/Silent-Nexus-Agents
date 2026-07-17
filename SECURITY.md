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
4. **Explicit approval.** Side-effecting actions ask unless safely configured;
   destructive/external actions cannot be auto-allowed. Raw shell, interpreters,
   wrappers, unproved commands, and approval-only host terminal execution are
   always one-time attended approvals.
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

- **Choose a real sandbox for untrusted code.** The process backend is an
  approval-only host guardrail, not containment. Use the pinned container
  backend for automatic or hostile/model-generated terminal execution. Check
  `snx sandbox status`.
- **Keep secrets in environment variables**, referenced by `api_key_env`.
  Never inline keys in config.
- **Read approval prompts.** They state the tool, risk, exact command/paths, and
  whether the action is being isolated.
- **Review the audit log** (`snx audit`) after unattended runs.

## Reporting a vulnerability

Report suspected vulnerabilities through the repository's
[private vulnerability reporting channel](https://github.com/Silent-Protocol-Inc/Silent-Nexus-Agents/security/advisories/new).
Include a reproduction, impact, and the affected version (`snx --version`).
Please do not open public issues for undisclosed vulnerabilities.

Supported security line: `1.x`. The most recent `1.x` release receives security
fixes. Experimental platforms may be asked to reproduce on the certified Linux
x86-64 target.
