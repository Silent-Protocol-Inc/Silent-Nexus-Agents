# Sandbox security

Silent Nexus never claims isolation it is not providing. `snx sandbox status`
prints the **actual** backend and an honest `IsolationReport`; the same report
is surfaced in every approval prompt so you always know whether an action is
being isolated.

## Backends

| Backend | `sandbox.backend` | Isolation level | What it isolates | What it does NOT isolate |
|---|---|---|---|---|
| Container | `container` / `auto` | `container` | Read-only rootfs, `--network none`, `--cap-drop ALL`, resource limits | Kernel bugs (shared kernel) |
| Restricted process | `process` / `auto`-fallback | `process-restricted` | Workspace path confinement, rlimits (CPU, address space, process count), wall-clock timeout, process-group kill, optional network namespace | Host filesystem visibility, kernel attack surface |
| None | `none` | `path-validation-only` | Workspace path checks only | Everything else — commands run as ordinary local processes |
| Mock | (tests) | `mock` | Nothing; returns canned outcomes | Everything |

With `auto` (the default), Silent Nexus tries the container backend first and
falls back to the restricted process backend if neither Docker nor Podman is
available — recording *why* in the selection notes shown by `snx sandbox status`.

## Restricted-process details

- Runs the command with no shell interpretation unless the operator explicitly
  approved a raw command line.
- Applies `RLIMIT_CPU`, `RLIMIT_AS`, and `RLIMIT_NPROC` (computed with headroom
  from the current UID limit so the sandbox never lowers below what is already
  running).
- Starts a new session/process group and kills the whole group on timeout or
  output-cap breach.
- When user+network namespaces are available, unshares the network namespace so
  the process has no interfaces (`network = off`). This is best-effort and
  reported honestly if unavailable.
- Scrubs the environment to a configured allowlist and always drops
  sensitive-looking variables.

## Guidance

- Treat `process-restricted` as a guardrail against *accidental* damage and
  runaway resource use, **not** as containment for hostile code.
- For untrusted or model-generated code you actually intend to execute, install
  Docker or Podman and use `sandbox.backend = "container"`.
- Never set `sandbox.backend = "none"` for anything but fully trusted commands
  in a throwaway environment.
