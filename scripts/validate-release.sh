#!/usr/bin/env bash
# Validate archive paths, contents, checksums, metadata, and executable smoke.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="$(python3 -c 'import pathlib,tomllib,sys; print(tomllib.loads(pathlib.Path(sys.argv[1]).read_text())["workspace"]["package"]["version"])' "$ROOT/Cargo.toml")"
ARCHIVE="${1:-$ROOT/dist/silent-nexus-${VERSION}-x86_64-unknown-linux-gnu.tar.gz}"
[[ -f "$ARCHIVE" ]] || { echo "error: archive not found: $ARCHIVE" >&2; exit 1; }

if tar -tzf "$ARCHIVE" | grep -Eq '(^/|(^|/)\.\.(/|$))'; then
  echo "error: archive contains an absolute or parent-traversal path" >&2
  exit 1
fi
if tar -tvzf "$ARCHIVE" | awk '$1 ~ /^[lh]/ { found=1 } END { exit !found }'; then
  echo "error: archive contains a symbolic or hard link" >&2
  exit 1
fi

TEMP="$(mktemp -d)"
trap 'rm -rf "$TEMP"' EXIT
tar -xzf "$ARCHIVE" -C "$TEMP"
PACKAGE_ROOT="$(find "$TEMP" -mindepth 1 -maxdepth 1 -type d -name "silent-nexus-${VERSION}-*" -print -quit)"
[[ -n "$PACKAGE_ROOT" ]] || { echo "error: package root missing" >&2; exit 1; }

for required in \
  snx LICENSE NOTICE README.md snx.1 SBOM.spdx.json SHA256SUMS \
  completions/snx.bash completions/_snx completions/snx.fish; do
  [[ -f "$PACKAGE_ROOT/$required" ]] || {
    echo "error: package is missing $required" >&2
    exit 1
  }
done
[[ -x "$PACKAGE_ROOT/snx" ]] || { echo "error: packaged snx is not executable" >&2; exit 1; }

(
  cd "$PACKAGE_ROOT"
  sha256sum -c SHA256SUMS
)
"$ROOT/scripts/validate-spdx.py" "$PACKAGE_ROOT/SBOM.spdx.json"
"$PACKAGE_ROOT/snx" --version | grep -F "$VERSION" >/dev/null
"$PACKAGE_ROOT/snx" --no-color about --compact | grep -F "x86_64-unknown-linux-gnu" >/dev/null

if [[ -f "$(dirname "$ARCHIVE")/SHA256SUMS" ]]; then
  (
    cd "$(dirname "$ARCHIVE")"
    grep -F "  $(basename "$ARCHIVE")" SHA256SUMS | sha256sum -c -
  )
fi

if [[ -x "${CARGO_TARGET_DIR:-$ROOT/target}/release/snx" ]]; then
  BUILD_HASH="$(sha256sum "${CARGO_TARGET_DIR:-$ROOT/target}/release/snx" | awk '{print $1}')"
  PACKAGE_HASH="$(sha256sum "$PACKAGE_ROOT/snx" | awk '{print $1}')"
  [[ "$BUILD_HASH" == "$PACKAGE_HASH" ]] || {
    echo "error: build and packaged binary hashes differ" >&2
    exit 1
  }
fi

echo "release validation: archive, manifest, SBOM, metadata, and binary are valid"
