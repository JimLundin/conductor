#!/usr/bin/env bash
# Clippy for the crates that build for the browser.
set -uo pipefail
cd "$(dirname "$0")/.."
status=0
# In the browser iroh's futures are not Send, while natively they are and
# future_not_send holds. The standard bans a target-conditional #[expect], so
# the wasm build alone allows that one lint.
wasm=(--target wasm32-unknown-unknown -- -A clippy::future_not_send)
for crate in proto client net; do
	cargo clippy -q -p "$crate" "${wasm[@]}" || status=1
done
cargo clippy -q -p app --features web "${wasm[@]}" || status=1
if ((status == 0)); then
	echo "proto, client, net and app pass for wasm32-unknown-unknown"
fi
exit "$status"
