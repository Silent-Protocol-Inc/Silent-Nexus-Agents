#!/usr/bin/env bash
# Exercise the committed source from a Git archive without using a remote.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

git diff --quiet
git diff --cached --quiet
[[ -z "$(git ls-files --others --exclude-standard)" ]] || {
  echo "error: clean-checkout smoke requires no untracked files" >&2
  exit 1
}

COMMIT="$(git rev-parse --verify HEAD)"
EPOCH="$(git log -1 --format=%ct)"
TEMP="$(mktemp -d)"
trap 'rm -rf "$TEMP"' EXIT
SOURCE="$TEMP/source"
CONFIG_HOME="$TEMP/config"
MOCK_WORKSPACE="$TEMP/mock-workspace"
mkdir -p "$SOURCE" "$CONFIG_HOME" "$MOCK_WORKSPACE/.nexus"
git archive --format=tar HEAD | tar -xf - -C "$SOURCE"

export CARGO_TARGET_DIR="$ROOT/target"
export SNX_BUILD_COMMIT="$COMMIT"
export SOURCE_DATE_EPOCH="$EPOCH"
(
  cd "$SOURCE"
  cargo test --workspace --locked --offline
  cargo build --release --locked -p nexus-cli
)

BIN="$CARGO_TARGET_DIR/release/snx"
"$BIN" --version | grep -F "1.0.0" >/dev/null
"$BIN" --no-color about --compact | grep -F "$COMMIT" >/dev/null
test ! -e "$SOURCE/.nexus"
(
  cd "$SOURCE"
  XDG_CONFIG_HOME="$CONFIG_HOME" "$BIN" config schema >"$TEMP/config.schema.json"
)
python3 -m json.tool "$TEMP/config.schema.json" >/dev/null
test ! -e "$SOURCE/.nexus"

XDG_CONFIG_HOME="$CONFIG_HOME" "$BIN" completion bash >"$TEMP/snx.bash"
XDG_CONFIG_HOME="$CONFIG_HOME" "$BIN" completion zsh >"$TEMP/_snx"
XDG_CONFIG_HOME="$CONFIG_HOME" "$BIN" completion fish >"$TEMP/snx.fish"
test -s "$TEMP/snx.bash"
test -s "$TEMP/_snx"
test -s "$TEMP/snx.fish"

(
  cd "$SOURCE"
  XDG_CONFIG_HOME="$CONFIG_HOME" "$BIN" --json sandbox status >"$TEMP/sandbox.json"
  XDG_CONFIG_HOME="$CONFIG_HOME" "$BIN" --json doctor --deep >"$TEMP/doctor.json"
  XDG_CONFIG_HOME="$CONFIG_HOME" "$BIN" --json maintenance check >"$TEMP/maintenance.json"
)

cat >"$MOCK_WORKSPACE/.nexus/config.toml" <<'EOF'
version = 1

[models.mock]
provider = "mock"
model = "mock"
role = "executor"
context_window = 8192
max_output_tokens = 1024
timeout_secs = 30

[routing]
simple = "mock"
coding = "mock"
planning = "mock"
fallback = "mock"

[sandbox]
backend = "process"
container_image = "debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818"
network = "off"
EOF
(
  cd "$MOCK_WORKSPACE"
  XDG_CONFIG_HOME="$CONFIG_HOME" "$BIN" --json run \
    --agent researcher "return a deterministic offline smoke response" \
    >"$TEMP/mock-flow.json"
)
grep -F "mock script exhausted" "$TEMP/mock-flow.json" >/dev/null

OUT_DIR="$ROOT/dist/clean-checkout" "$SOURCE/scripts/package-release.sh"
"$SOURCE/scripts/validate-release.sh" \
  "$ROOT/dist/clean-checkout/silent-nexus-1.0.0-x86_64-unknown-linux-gnu.tar.gz"

echo "clean-checkout smoke: tests, release build, diagnostics, completions, and mock flow passed"
