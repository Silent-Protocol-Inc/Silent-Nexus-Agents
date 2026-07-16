# Release process

The local release process is evidence-driven and never configures a remote,
pushes, publishes crates, contacts model providers, or creates a hosted release.

## Required state

- branch `main`;
- exactly the baseline import and release commits for the 1.0.0 certification;
- clean worktree and index;
- no configured Git remote;
- Rust `1.97.0`;
- version `1.0.0`;
- migrations `0001` through `0004` unchanged and `0005` appended;
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

`scripts/package-release.sh` creates:

```text
dist/silent-nexus-1.0.0-x86_64-unknown-linux-gnu.tar.gz
dist/SHA256SUMS
```

The archive is reproducible for the same committed source, Rust toolchain,
lockfile, build commit, and source epoch. It contains the binary, license,
README, man page, completions, SPDX SBOM, and internal `SHA256SUMS`.

Validate independently with `scripts/validate-release.sh ARCHIVE`.

## Tag and installation

After every gate passes, create annotated tag `v1.0.0`. Install the exact
verified packaged/build binary atomically, then compare SHA-256 across build,
package, installed file, and compatibility symlink target. Confirm root
ownership, mode `0755`, version `1.0.0`, and no remote.

Rollback means restoring the previous verified binary; schema rollback requires
the pre-upgrade backup.
