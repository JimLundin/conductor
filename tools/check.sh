#!/usr/bin/env bash
# The gate every commit passes: tests, clippy (native and wasm), formatting.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
WASM_CRATES=(proto client)
cargo fmt --check
# Run records name the host's public address; it must never be committed.
python3 tools/scrub-ips.py --check $(git ls-files 'docs/**/*.txt' 'docs/**/*.log' 'docs/**/NOTES.md')
cargo clippy --workspace --all-targets -- -D warnings
for crate in "${WASM_CRATES[@]}"; do
  if [ -d "crates/$crate" ]; then
    cargo clippy -p "$crate" --target wasm32-unknown-unknown -- -D warnings
  fi
done
if [ -d crates/app ]; then
  cargo clippy -p app --features web --target wasm32-unknown-unknown -- -D warnings
fi
cargo test --workspace
# The subset checker, pinned in tools/subset. SUBSET=0 skips it (it builds
# the checker on first use, which needs the network).
if [ "${SUBSET:-1}" = 1 ]; then
  tools/subset check
fi
