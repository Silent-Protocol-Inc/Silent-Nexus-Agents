#!/usr/bin/env bash
# Deterministic local production gate. It never pushes or publishes.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
VERSION="$(python3 -c 'import pathlib,tomllib; print(tomllib.loads(pathlib.Path("Cargo.toml").read_text())["workspace"]["package"]["version"])' </dev/null)"
TARGET="x86_64-unknown-linux-gnu"
ARCHIVE="dist/silent-nexus-${VERSION}-${TARGET}.tar.gz"
CLEAN_ARCHIVE="dist/clean-checkout/silent-nexus-${VERSION}-${TARGET}.tar.gz"

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
[[ "$(git branch --show-current)" == "main" ]] || {
  echo "error: release must be checked on main" >&2
  exit 1
}

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
sha256sum -c <<'EOF'
2e539861a8f1c962a0b976012277eaf86f268fef675c94b1ef114d2670c6b5ef  migrations/0001_initial.sql
fb08343841693564462a9d9c3e53b7da21171e0ef5c1a044c632ece773edd263  migrations/0002_interactive_agent.sql
cb126d62a9d6490b5ca614b06ccc8b3886256dad07c652bb635d4d5807159c0e  migrations/0003_session_approval_grants.sql
f699ea59d2940f7e0c6435be7af6f72a74e6829720297b1b0d2500f4f965615c  migrations/0004_orchestration_timeline.sql
d09f95133e1858237e55132784d4f6a2bdd5ed6260a6438439a70c452ce1f4d9  migrations/0005_production_hardening.sql
EOF

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
# The committed config schema is a published artifact, but nothing regenerates
# it. Two consecutive releases shipped a schema missing a whole config block, so
# the gate now fails when it drifts from what the binary actually generates.
cargo run --quiet --locked --offline -p nexus-cli -- config schema \
  > target/config.schema.generated.json
diff -u schemas/config.schema.json target/config.schema.generated.json || {
  echo "error: schemas/config.schema.json is stale; regenerate with:" >&2
  echo "       snx config schema > schemas/config.schema.json" >&2
  exit 1
}
echo "config schema: committed copy matches the generated schema"

scripts/secret-scan.sh
scripts/generate-spdx.py target/silent-nexus.spdx.json
scripts/validate-spdx.py target/silent-nexus.spdx.json
scripts/package-release.sh
FIRST_ARCHIVE_SHA="$(
  sha256sum "$ARCHIVE" | awk '{print $1}'
)"
scripts/package-release.sh
SECOND_ARCHIVE_SHA="$(
  sha256sum "$ARCHIVE" | awk '{print $1}'
)"
[[ "$FIRST_ARCHIVE_SHA" == "$SECOND_ARCHIVE_SHA" ]] || {
  echo "error: release archive is not deterministic" >&2
  exit 1
}
scripts/validate-release.sh "$ARCHIVE"
scripts/clean-checkout-smoke.sh
MAIN_ARCHIVE_SHA="$(
  sha256sum "$ARCHIVE" | awk '{print $1}'
)"
CLEAN_ARCHIVE_SHA="$(
  sha256sum "$CLEAN_ARCHIVE" | awk '{print $1}'
)"
[[ "$MAIN_ARCHIVE_SHA" == "$CLEAN_ARCHIVE_SHA" ]] || {
  echo "error: clean-checkout release archive differs from the primary archive" >&2
  exit 1
}

echo "release check: every local $VERSION production gate passed"
