#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
git ls-files -z -- '*.rs' | xargs -0 sed -i -E '/^[[:space:]]*\/\/[/!]/d'
cargo fmt
