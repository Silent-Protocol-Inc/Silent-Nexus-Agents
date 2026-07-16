#!/usr/bin/env bash
# Deterministic local production gate. It never pushes or publishes.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

for command in cargo rustc git python3 sha256sum tar gzip; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "error: required command is missing: $command" >&2
    exit 1
  }
done
for cargo_command in audit deny; do
  cargo "$cargo_command" --version >/dev/null 2>&1 || {
    echo "error: cargo-$cargo_command is required for release checks" >&2
    exit 1
  }
done
[[ "$(cargo audit --version)" == *" 0.22.2" ]] || {
  echo "error: release requires cargo-audit 0.22.2; run scripts/install-release-tools.sh" >&2
  exit 1
}
[[ "$(cargo deny --version)" == "cargo-deny 0.20.2" ]] || {
  echo "error: release requires cargo-deny 0.20.2; run scripts/install-release-tools.sh" >&2
  exit 1
}

git diff --quiet
git diff --cached --quiet
[[ -z "$(git ls-files --others --exclude-standard)" ]] || {
  echo "error: release checks require a clean tree" >&2
  exit 1
}
[[ -z "$(git remote)" ]] || {
  echo "error: release repository must not have a configured remote" >&2
  exit 1
}
[[ "$(git branch --show-current)" == "main" ]] || {
  echo "error: release must be checked on main" >&2
  exit 1
}
[[ "$(git rev-list --count HEAD)" -eq 2 ]] || {
  echo "error: the certified repository must contain exactly the baseline and release commits" >&2
  exit 1
}
[[ "$(git log -1 --format=%s HEAD~1)" == "chore: import Silent Nexus 0.2.0 baseline" ]]
[[ "$(git log -1 --format=%s HEAD)" == "feat!: harden Silent Nexus for 1.0.0" ]]

for ignored in \
  target/.release-ignore-probe \
  .nexus/.release-ignore-probe \
  dist/.release-ignore-probe \
  release/.release-ignore-probe \
  coverage/.release-ignore-probe \
  .env \
  credentials.json; do
  git check-ignore --no-index -q "$ignored" || {
    echo "error: expected ignored path is not covered: $ignored" >&2
    exit 1
  }
done
git diff --quiet HEAD~1 -- \
  migrations/0001_initial.sql \
  migrations/0002_interactive_agent.sql \
  migrations/0003_session_approval_grants.sql \
  migrations/0004_orchestration_timeline.sql

export SNX_BUILD_COMMIT="$(git rev-parse --verify HEAD)"
export SOURCE_DATE_EPOCH="$(git log -1 --format=%ct)"
export CARGO_INCREMENTAL=0

cargo fetch --locked
cargo metadata --format-version 1 --locked --offline >/dev/null
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked --offline -- -D warnings
cargo test --workspace --locked --offline
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked --offline
cargo audit
cargo deny check
mkdir -p target
cargo tree -d --locked > target/release-dependency-duplicates.txt
scripts/secret-scan.sh
scripts/generate-spdx.py target/silent-nexus.spdx.json
scripts/validate-spdx.py target/silent-nexus.spdx.json
scripts/package-release.sh
FIRST_ARCHIVE_SHA="$(
  sha256sum dist/silent-nexus-1.0.0-x86_64-unknown-linux-gnu.tar.gz | awk '{print $1}'
)"
scripts/package-release.sh
SECOND_ARCHIVE_SHA="$(
  sha256sum dist/silent-nexus-1.0.0-x86_64-unknown-linux-gnu.tar.gz | awk '{print $1}'
)"
[[ "$FIRST_ARCHIVE_SHA" == "$SECOND_ARCHIVE_SHA" ]] || {
  echo "error: release archive is not deterministic" >&2
  exit 1
}
scripts/validate-release.sh dist/silent-nexus-1.0.0-x86_64-unknown-linux-gnu.tar.gz
scripts/clean-checkout-smoke.sh
MAIN_ARCHIVE_SHA="$(
  sha256sum dist/silent-nexus-1.0.0-x86_64-unknown-linux-gnu.tar.gz | awk '{print $1}'
)"
CLEAN_ARCHIVE_SHA="$(
  sha256sum dist/clean-checkout/silent-nexus-1.0.0-x86_64-unknown-linux-gnu.tar.gz \
    | awk '{print $1}'
)"
[[ "$MAIN_ARCHIVE_SHA" == "$CLEAN_ARCHIVE_SHA" ]] || {
  echo "error: clean-checkout release archive differs from the primary archive" >&2
  exit 1
}

echo "release check: every local 1.0.0 production gate passed"
