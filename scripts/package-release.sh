#!/usr/bin/env bash
# Build a deterministic, checksummed Linux x86-64 Silent Nexus release archive.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
OUT_DIR="${OUT_DIR:-$ROOT/dist}"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
if [[ "$TARGET_DIR" != /* ]]; then
  TARGET_DIR="$ROOT/$TARGET_DIR"
fi
VERSION="$(python3 -c 'import pathlib,tomllib; print(tomllib.loads(pathlib.Path("Cargo.toml").read_text())["workspace"]["package"]["version"])' </dev/null)"
TARGET="$(rustc -vV | sed -n 's/^host: //p')"

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]]; then
  echo "error: invalid workspace package version in Cargo.toml: $VERSION" >&2
  exit 1
fi
if [[ "$TARGET" != "x86_64-unknown-linux-gnu" ]]; then
  echo "error: certified packaging target is x86_64-unknown-linux-gnu; found $TARGET" >&2
  exit 1
fi

COMMIT="${SNX_BUILD_COMMIT:-$(git -C "$ROOT" rev-parse --verify HEAD 2>/dev/null || printf development)}"
EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$ROOT" log -1 --format=%ct 2>/dev/null || printf 0)}"
NAME="silent-nexus-${VERSION}-${TARGET}"
ARCHIVE="$OUT_DIR/${NAME}.tar.gz"

mkdir -p "$OUT_DIR"
rm -f "$ARCHIVE" "$OUT_DIR/SHA256SUMS"
STAGING="$(mktemp -d "$OUT_DIR/.stage.XXXXXX")"
trap 'rm -rf "$STAGING"' EXIT
PACKAGE_ROOT="$STAGING/$NAME"
mkdir -p "$PACKAGE_ROOT/completions"

(
  cd "$ROOT"
  cargo fetch --locked
  SNX_BUILD_COMMIT="$COMMIT" SOURCE_DATE_EPOCH="$EPOCH" \
    cargo build --release --locked -p nexus-cli
)

BIN="$TARGET_DIR/release/snx"
[[ -x "$BIN" ]] || { echo "error: release binary missing: $BIN" >&2; exit 1; }
install -m 0755 "$BIN" "$PACKAGE_ROOT/snx"
install -m 0644 "$ROOT/LICENSE" "$PACKAGE_ROOT/LICENSE"
install -m 0644 "$ROOT/README.md" "$PACKAGE_ROOT/README.md"
install -m 0644 "$ROOT/man/snx.1" "$PACKAGE_ROOT/snx.1"
"$BIN" completion bash >"$PACKAGE_ROOT/completions/snx.bash"
"$BIN" completion zsh >"$PACKAGE_ROOT/completions/_snx"
"$BIN" completion fish >"$PACKAGE_ROOT/completions/snx.fish"

SNX_BUILD_COMMIT="$COMMIT" SOURCE_DATE_EPOCH="$EPOCH" \
  "$ROOT/scripts/generate-spdx.py" "$PACKAGE_ROOT/SBOM.spdx.json"
"$ROOT/scripts/validate-spdx.py" "$PACKAGE_ROOT/SBOM.spdx.json"

(
  cd "$PACKAGE_ROOT"
  find . -type f ! -name SHA256SUMS -print0 \
    | LC_ALL=C sort -z \
    | xargs -0 sha256sum \
    | sed 's#  \./#  #' >SHA256SUMS
)

tar \
  --sort=name \
  --format=posix \
  --pax-option=delete=atime,delete=ctime \
  --mtime="@$EPOCH" \
  --owner=0 \
  --group=0 \
  --numeric-owner \
  -C "$STAGING" \
  -cf - "$NAME" \
  | gzip -n -9 >"$ARCHIVE"

(
  cd "$OUT_DIR"
  sha256sum "$(basename "$ARCHIVE")" >SHA256SUMS
)

echo "release archive: $ARCHIVE"
echo "release sha256: $(sha256sum "$ARCHIVE" | awk '{print $1}')"
