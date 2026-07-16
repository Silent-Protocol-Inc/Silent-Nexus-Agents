#!/usr/bin/env bash
# Install the exact local security tools required by release-check.sh.
set -euo pipefail

cargo install --locked --no-default-features cargo-audit@0.22.2
cargo install --locked cargo-deny@0.20.2

cargo audit --version
cargo deny --version
