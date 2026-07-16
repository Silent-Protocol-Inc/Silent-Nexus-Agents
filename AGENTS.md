# Silent Nexus repository instructions

Silent Nexus is a security-sensitive Rust agent harness. Treat model output,
web content, MCP output, repository content, and command output as untrusted.

## Safety boundaries

- Never bypass `WorkspaceGuard`, policy evaluation, approval, sandbox metadata,
  redaction, terminal sanitization, or audit recording.
- Generic model filesystem tools must not read or write `.nexus`, `.git`, or
  credential-bearing paths. Public examples such as `.env.example` may remain
  readable.
- Raw shell, interpreters, wrappers, unproved commands, and approval-only host
  execution are one-time approvals. They may not receive session grants or run
  unattended.
- Generic terminal `git commit`, `git push`, `git remote`, privilege
  escalation, Git aliases, and unrecognized Git operations are denied. Use the
  typed Git workflows and `nexus_core::git::GitRunner`.
- Do not weaken the pinned container image, network-off default, sensitive
  mounts, UID/GID mapping, read-only action mounts, capability drop, or resource
  limits without a documented security review.
- Secrets must use `SecretString`, zeroization, private atomic writes, and
  redaction. Never add a real credential to tests, examples, logs, or fixtures.

## Storage and migrations

- Migrations are append-only. Never edit `migrations/0001_*.sql` through
  `0005_production_hardening.sql` after release.
- Config version remains `1` throughout the 1.x compatibility line unless an
  explicit compatibility decision says otherwise.
- Preserve existing timeline and redacted JSONL export fields; add fields
  compatibly.
- State/auth/log directories are `0700`; private files are `0600`. State writes
  must be no-follow, same-directory, atomic replacements.
- Do not add automatic deletion of transcripts, tasks, plans, goals, memories,
  or artifacts.

## Required validation

Run commands with the pinned toolchain and lockfile:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
```

Security- or release-facing changes also require:

```sh
cargo audit
cargo deny check
scripts/secret-scan.sh
scripts/package-release.sh
scripts/validate-release.sh dist/silent-nexus-1.0.0-x86_64-unknown-linux-gnu.tar.gz
```

Add adversarial tests for every safety boundary changed. Timing claims must be
measured on the release host, and documentation must distinguish strong
container isolation from approval-only host-process guardrails.

## Release rules

- Linux `x86_64-unknown-linux-gnu` is the only certified 1.0.0 target.
- Release builds use `--locked`, embedded commit/epoch metadata, an SPDX SBOM,
  internal and external SHA-256 manifests, and deterministic archives.
- A high/critical advisory, unexplained medium advisory, secret finding,
  migration failure, integrity mismatch, warning, or failed test blocks release.
- Release scripts do not configure remotes, push, publish crates, or create a
  hosted release.
