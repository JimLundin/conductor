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
for crate in proto client; do
	cargo clippy -q -p "$crate" --target wasm32-unknown-unknown || status=1
done
cargo clippy -q -p app --features web --target wasm32-unknown-unknown || status=1
exit "$status"
