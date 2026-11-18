#!/usr/bin/env bash
# The gate: the shared Rust standard, which runs Conductor's suites from
# checks.toml beside its own checks. `tools/check.sh [commit|step|release]
# [--base <ref>|--all] [--no-cache]`.
set -uo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
exec "${STANDARDS:-/mnt/c/Users/Jim.Lundin/Projects/standards}/rust/check.sh" "$@"
