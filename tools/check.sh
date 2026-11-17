#!/usr/bin/env bash
# The gate every commit passes: the shared Rust standard, then what only
# Conductor needs. `tools/check.sh full` runs the standard's full profile.
set -uo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
standards=${STANDARDS:-/mnt/c/Users/Jim.Lundin/Projects/standards}
status=0
"$standards/rust/check.sh" "$@" || status=1
# Run records name the host's public address; it must never be committed.
mapfile -t records < <(git ls-files 'docs/**/*.txt' 'docs/**/*.log' 'docs/**/NOTES.md')
python3 tools/scrub-ips.py --check "${records[@]}" || status=1
# In the browser iroh's futures are not Send, while natively they are and
# future_not_send holds. The standard bans a target-conditional #[expect], so
# the wasm build alone allows that one lint.
wasm=(--target wasm32-unknown-unknown -- -A clippy::future_not_send)
for crate in proto client net; do
	cargo clippy -q -p "$crate" "${wasm[@]}" || status=1
done
cargo clippy -q -p app --features web "${wasm[@]}" || status=1
exit "$status"
