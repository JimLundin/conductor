#!/usr/bin/env bash
# The gate every commit passes: tests, clippy (native and wasm), formatting.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
WASM_CRATES=(proto client app)
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
for crate in "${WASM_CRATES[@]}"; do
  if [ -d "crates/$crate" ]; then
    cargo clippy -p "$crate" --target wasm32-unknown-unknown -- -D warnings
  fi
done
cargo test --workspace
