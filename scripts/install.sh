#!/usr/bin/env bash
#
# Build NEXUS by Silent Protocol and install the `snx` binary.
# Does not download models, deploy anything, or touch system directories
# outside the chosen prefix.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${PREFIX:-$HOME/.local/bin}"

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo not found. Install Rust stable (1.97+): https://rustup.rs" >&2
  exit 1
fi

echo "Building NEXUS by Silent Protocol (release) ..."
( cd "$ROOT" && cargo build --release -p nexus-cli )

BIN="$ROOT/target/release/snx"
if [[ ! -x "$BIN" ]]; then
  echo "error: build did not produce $BIN" >&2
  exit 1
fi

mkdir -p "$PREFIX"
install -m 0755 "$BIN" "$PREFIX/snx"
echo "Installed snx -> $PREFIX/snx"
"$PREFIX/snx" --no-color about --compact --brand-only

if ! printf '%s' ":$PATH:" | grep -q ":$PREFIX:"; then
  echo "note: $PREFIX is not on your PATH. Add it, e.g.:"
  echo "      export PATH=\"$PREFIX:\$PATH\""
fi

echo "Run 'snx doctor' to check readiness."
