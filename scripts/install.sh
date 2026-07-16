#!/usr/bin/env bash
# Install or uninstall Silent Nexus without changing user data.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="user"
PREFIX=""
BINARY=""
DRY_RUN=0
UNINSTALL=0

usage() {
  cat <<'EOF'
Usage: scripts/install.sh [options]

Options:
  --user             Install under ~/.local (default)
  --system           Install under /usr/local; run as root
  --prefix PATH      Install under a custom prefix
  --binary PATH      Install an existing snx binary instead of building
  --dry-run          Print intended changes without writing
  --uninstall        Remove installed program files; preserve all user state
  -h, --help         Show this help
EOF
}

while (($#)); do
  case "$1" in
    --user)
      MODE="user"
      ;;
    --system)
      MODE="system"
      ;;
    --prefix)
      shift
      [[ $# -gt 0 ]] || { echo "error: --prefix needs a path" >&2; exit 2; }
      PREFIX="$1"
      MODE="custom"
      ;;
    --binary)
      shift
      [[ $# -gt 0 ]] || { echo "error: --binary needs a path" >&2; exit 2; }
      BINARY="$1"
      ;;
    --dry-run)
      DRY_RUN=1
      ;;
    --uninstall)
      UNINSTALL=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

if [[ -z "$PREFIX" ]]; then
  if [[ "$MODE" == "system" ]]; then
    PREFIX="/usr/local"
  else
    PREFIX="${HOME:?HOME is not set}/.local"
  fi
fi
PREFIX="${PREFIX%/}"

if [[ "$MODE" == "system" && "$DRY_RUN" -eq 0 && "${EUID:-$(id -u)}" -ne 0 ]]; then
  echo "error: --system writes under /usr/local; rerun this script as root" >&2
  exit 1
fi

BIN_TARGET="$PREFIX/bin/snx"
MAN_TARGET="$PREFIX/share/man/man1/snx.1"
BASH_TARGET="$PREFIX/share/bash-completion/completions/snx"
ZSH_TARGET="$PREFIX/share/zsh/site-functions/_snx"
FISH_TARGET="$PREFIX/share/fish/vendor_completions.d/snx.fish"

run() {
  if [[ "$DRY_RUN" -eq 1 ]]; then
    printf '+'
    printf ' %q' "$@"
    printf '\n'
  else
    "$@"
  fi
}

remove_file() {
  if [[ -e "$1" || -L "$1" ]]; then
    run rm -f -- "$1"
  fi
}

if [[ "$UNINSTALL" -eq 1 ]]; then
  remove_file "$BIN_TARGET"
  remove_file "$MAN_TARGET"
  remove_file "$BASH_TARGET"
  remove_file "$ZSH_TARGET"
  remove_file "$FISH_TARGET"
  echo "Silent Nexus program files removed from $PREFIX."
  echo "Workspace .nexus directories and user configuration were preserved."
  exit 0
fi

if [[ -z "$BINARY" ]]; then
  command -v cargo >/dev/null 2>&1 || {
    echo "error: cargo is required when --binary is not supplied" >&2
    exit 1
  }
  BUILD_COMMIT="$(git -C "$ROOT" rev-parse --verify HEAD 2>/dev/null || printf development)"
  BUILD_EPOCH="$(git -C "$ROOT" log -1 --format=%ct 2>/dev/null || date +%s)"
  if [[ "$DRY_RUN" -eq 1 ]]; then
    echo "+ SNX_BUILD_COMMIT=$BUILD_COMMIT SOURCE_DATE_EPOCH=$BUILD_EPOCH cargo build --release --locked -p nexus-cli"
  else
    (
      cd "$ROOT"
      SNX_BUILD_COMMIT="$BUILD_COMMIT" SOURCE_DATE_EPOCH="$BUILD_EPOCH" \
        cargo build --release --locked -p nexus-cli
    )
  fi
  BINARY="${CARGO_TARGET_DIR:-$ROOT/target}/release/snx"
fi

if [[ "$DRY_RUN" -eq 0 ]]; then
  BINARY="$(cd "$(dirname "$BINARY")" && pwd)/$(basename "$BINARY")"
  [[ -f "$BINARY" && -x "$BINARY" ]] || {
    echo "error: --binary must name an executable regular file: $BINARY" >&2
    exit 1
  }
  "$BINARY" --version | grep -F "1.0.0" >/dev/null || {
    echo "error: the supplied binary is not Silent Nexus 1.0.0" >&2
    exit 1
  }
fi

install_atomic() {
  local source="$1"
  local target="$2"
  local mode="$3"
  local directory
  local temporary
  directory="$(dirname "$target")"
  temporary="$directory/.snx-install-$$-$(basename "$target")"
  run install -d -m 0755 "$directory"
  run install -m "$mode" "$source" "$temporary"
  run mv -f -- "$temporary" "$target"
}

if [[ "$DRY_RUN" -eq 1 ]]; then
  echo "+ generate shell completions from $BINARY"
else
  COMPLETION_DIR="$(mktemp -d)"
  trap 'rm -rf "$COMPLETION_DIR"' EXIT
  "$BINARY" completion bash >"$COMPLETION_DIR/snx.bash"
  "$BINARY" completion zsh >"$COMPLETION_DIR/_snx"
  "$BINARY" completion fish >"$COMPLETION_DIR/snx.fish"
fi

install_atomic "$BINARY" "$BIN_TARGET" 0755
install_atomic "$ROOT/man/snx.1" "$MAN_TARGET" 0644
if [[ "$DRY_RUN" -eq 0 ]]; then
  install_atomic "$COMPLETION_DIR/snx.bash" "$BASH_TARGET" 0644
  install_atomic "$COMPLETION_DIR/_snx" "$ZSH_TARGET" 0644
  install_atomic "$COMPLETION_DIR/snx.fish" "$FISH_TARGET" 0644
else
  echo "+ install generated Bash completion -> $BASH_TARGET"
  echo "+ install generated Zsh completion -> $ZSH_TARGET"
  echo "+ install generated Fish completion -> $FISH_TARGET"
fi

echo "Installed Silent Nexus -> $BIN_TARGET"
if [[ "$DRY_RUN" -eq 0 ]]; then
  "$BIN_TARGET" --no-color about --compact --brand-only
fi
if ! printf '%s' ":$PATH:" | grep -Fq ":$PREFIX/bin:"; then
  echo "Add $PREFIX/bin to PATH."
fi
echo "Run: snx doctor --deep"
