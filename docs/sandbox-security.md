# Sandbox security

`snx sandbox status` reports the backend actually selected, its
`IsolationStrength`, filesystem access, network posture, resource controls, and
caveats. Approval prompts use the same facts.

## Backends

| Backend | Strength | Automatic model terminal execution | Boundary |
|---|---|---|---|
| Container | `strong` | Allowed by policy/approval | Ephemeral Docker/Podman container |
| Process | `approval_only_host` | Denied | Ordinary host process with guardrails |
| None | `none` | Denied | Path validation only |
| Mock | `mock` | Tests only | Deterministic test double |

`auto` checks Docker then Podman and selects a container only when the engine
responds and the exact pinned image already exists locally. Silent Nexus never
pulls an image implicitly. Otherwise `auto` reports the reason and selects the
approval-only host backend.

## Strong container contract

Each execution:

- uses the digest-pinned default image;
- runs as the invoking UID/GID;
- uses a read-only root filesystem and private `/tmp` tmpfs;
- mounts no workspace for `NoWorkspace`, mounts it read-only for reads, and
  writable only for an approved write;
- hides `.git`, `.nexus`, and detected credential/private-key paths with
  inaccessible mounts;
- refuses container execution if sensitive-path discovery cannot complete,
  rather than running with a partial or empty mask set;
- disables network by default and never exceeds the approved network mode;
- drops all capabilities, enables `no-new-privileges`, disables IPC sharing,
  and applies memory, CPU, PID, wall-clock, and shared output limits;
- disables the engine log driver so transient model output is retained only by
  the bounded Silent Nexus collector;
- kills the named container immediately on timeout or output-cap breach.

Containers share the host kernel and are not virtual machines. Keep the engine
patched and review image digest changes as supply-chain changes.

## Approval-only host contract

The process backend adds environment scrubbing, working-directory validation,
CPU/address-space/process limits, optional network namespace, timeout, shared
stdout/stderr cap, and process-group cleanup. Those are guardrails, not
containment: the process can still see host resources available to the user.

For every model-proposed terminal or command-backed repository action:

- policy is forced to a prominent one-time attended approval;
- session grants and auto-edit/full-access approval are unavailable;
- non-interactive `--yes`, background workers, and unattended approvers deny;
- an invocation-scoped unsafe-host token is consumed by that action and cleared
  defensively on early failure.

Raw shell, interpreters, wrappers, substitutions, and unproved commands are
also one-time only even under a container.

## Filesystem exclusions

Generic model filesystem tools deny `.nexus`, `.git`, `.env*` except the exact
public `.env.example`, `.netrc`, `.npmrc`, `.pypirc`, Git credential files,
cloud/Kubernetes/Docker/SSH/GPG credential roots, auth/credential JSON, private
keys, and operator-configured denied paths. Writes reject symlink leaves and
escapes.

Trusted typed Git workflows run through the sanitized bounded `GitRunner`
outside model containers so they can inspect Git metadata without exposing
`.git` to generic model tools.

## Network

`off` is the default. `restricted` means the network is reachable but
destination validation is enforced by the typed tool; it is not a generic
container egress firewall. `full` requires explicit configuration/approval.

Web tools separately enforce scheme, credential URL, private-range, metadata,
redirect, and DNS-rebinding protections.
