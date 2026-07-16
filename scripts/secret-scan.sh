#!/usr/bin/env bash
# Fail when tracked source contains common credential/private-key signatures.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

PATTERN='AKIA[0-9A-Z]{16}|ASIA[0-9A-Z]{16}|-----BEGIN ([A-Z0-9 ]+ )?PRIVATE KEY-----|gh[pousr]_[A-Za-z0-9_]{30,}|github_pat_[A-Za-z0-9_]{30,}|xox[baprs]-[A-Za-z0-9-]{20,}|sk-[A-Za-z0-9]{32,}'

if git grep -nIE "$PATTERN" -- . \
  ':(exclude)scripts/secret-scan.sh' \
  ':(exclude)docs/threat-model.md' \
  ':(exclude)crates/nexus-core/src/redact.rs'; then
  echo "error: tracked source matched a credential/private-key pattern" >&2
  exit 1
fi

echo "secret scan: no tracked credential signatures found"
