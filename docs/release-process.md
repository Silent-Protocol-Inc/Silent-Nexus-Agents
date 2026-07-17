# Release process

The local release process is evidence-driven and never pushes, publishes
crates, contacts model providers, or creates a hosted release. A configured
Git remote is allowed but is never used by the release scripts.

## Required state

- branch `main`;
- clean worktree and index;
- Rust `1.97.0`;
- a valid workspace version in `Cargo.toml`, used for every artifact check;
- migrations `0001` through `0005` match their certified baseline SHA-256
  checksums and later migrations are appended;
- `cargo-audit` and `cargo-deny` installed.

Install the pinned release tools with:

```sh
scripts/install-release-tools.sh
```

## Gates

Run:

```sh
scripts/release-check.sh
```

It verifies repository structure/ignores, formatting, Clippy with warnings
denied, locked tests, rustdoc, advisory audit, license/source/bans policy,
secret patterns, SPDX generation, deterministic packaging, archive checksums,
and a Git-archive clean-checkout smoke including:

- release build;
- `snx --version` and `about`;
- `doctor --deep` and `maintenance check`;
- sandbox status;
- Bash/Zsh/Fish completions;
- fresh-workspace offline mock-provider flow.

Any high/critical advisory, unexplained medium advisory, secret finding,
warning, migration/integrity failure, security regression, or failing test
blocks the tag.

## Artifact

For the workspace version `$VERSION`, `scripts/package-release.sh` creates:

```text
dist/silent-nexus-$VERSION-x86_64-unknown-linux-gnu.tar.gz
dist/SHA256SUMS
```

The archive is reproducible for the same committed source, Rust toolchain,
lockfile, build commit, and source epoch. It contains the binary, license,
README, man page, completions, SPDX SBOM, and internal `SHA256SUMS`.

Validate independently with `scripts/validate-release.sh ARCHIVE`.

## Tag and installation

After every gate passes, create annotated tag `v$VERSION`. Install the exact
verified packaged/build binary atomically, then compare SHA-256 across build,
package, installed file, and compatibility symlink target. Confirm root
ownership, mode `0755`, and the expected version.

Rollback means restoring the previous verified binary; schema rollback requires
the pre-upgrade backup.
