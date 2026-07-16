# Threat model

Silent Nexus assumes the model, retrieved content, repository content, tool
output, and provider responses may be malicious. The operator and local
installation are trusted; the harness, not the model, enforces safety.

## Protected assets

- host files and credentials outside the workspace;
- workspace code/data and Git history;
- `.nexus` state, transcripts, plans, goals, tasks, artifacts, and logs;
- provider tokens and credential profiles;
- network position and cloud metadata;
- terminal integrity and local compute;
- release/source/database integrity.

## Model action defenses

Unknown tools and malformed schemas fail closed. Role capabilities, workspace
path validation, policy, approvals, isolation metadata, output/time limits,
redaction, terminal sanitization, and audit records run outside the model.

Structured command analysis examines every shell chain/pipeline segment and
concrete argv. Raw shell, interpreters, wrappers, substitutions, and unproved
commands are destructive one-time approvals. Privilege escalation and generic
terminal Git commit/push/remote/alias/unrecognized operations are hard denied.

Session grants require proved, structured, non-destructive argv under strong
container isolation. Automatic/background terminal execution is denied without
that isolation.

## Filesystem and state defenses

`WorkspaceGuard` canonicalizes paths, rejects escapes and symlink writes, and
denies state/Git/credential paths to generic model tools. Containers mask
detected sensitive paths. Private state uses repaired `0700`/`0600`
permissions and no-follow same-directory atomic replacement.

Artifact reads validate state-root confinement, regular/no-follow file type,
size, and SHA-256. Migration source is embedded and checksummed; differing
applied history blocks database open.

## Process and container defenses

Strong containers run as the invoking UID/GID with per-action mounts,
read-only rootfs, hidden sensitive paths, network-off defaults, dropped
capabilities, resource limits, no daemon-side output logging, and immediate
container kill.

Host process execution is explicitly `approval_only_host`, not isolation.
Every model-proposed action requires attended one-time approval; unattended and
background execution is denied. Process groups are killed on timeout/output cap
and the authorization token cannot carry to another invocation.

## Network and content defenses

Web/MCP content is untrusted data and cannot become higher-priority
instructions. Web tools reject unsafe schemes, credential URLs, private and
metadata destinations, unapproved loopback, unsafe redirects, and DNS
rebinding. Network mode never exceeds the concrete action's approved mode.

## Secret and terminal defenses

Secrets use redacted serialization/debug output and audited zeroization.
Environment forwarding drops sensitive and interpreter/Git loader variables.
Redaction occurs before persistence/display, and memory refuses content that
would be redacted. Terminal sanitization strips CSI/OSC/DCS and carriage-return
spoofing.

## Resource exhaustion

SQLite has busy timeouts, bounded retries, WAL checkpointing, and indexed
status/search paths. Timeline search uses FTS and bounded results. Process and
container output share one byte budget; crossing it kills execution immediately
instead of waiting for timeout. TUI layout caches and visible-range rendering
avoid repeatedly wrapping the full transcript.

## Supply chain and release

Rust/MSRV, lockfile, container image, CI actions, internal crate publication,
license/source policy, advisory audit, secret scan, SPDX SBOM, deterministic
archive, and SHA-256 manifests are release gates. Only Linux x86-64 is
certified for 1.0.0.

## Residual risk and non-goals

- Containers share the host kernel.
- Approval-only host execution can access host resources available to the user.
- An operator can approve a damaging in-scope action.
- Silent Nexus does not defend a compromised host or malicious operator.
- `restricted` network mode relies on typed destination validation, not a
  generic egress proxy.
- No test suite proves the absence of unknown defects; production-ready means
  every discovered/reproducible issue and declared gate is resolved.
