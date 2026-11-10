#!/usr/bin/env bash
# Browser end to end through n0's relays: builds the web app, serves it under
# /conductor/ like GitHub Pages does, starts `conductord s3-host`, and drives
# headless Chromium and WebKit at an iPhone viewport. Output: docs/s3/browser-run.txt
# and docs/s3/iphone-390x844.png (from WebKit).
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH" NO_COLOR=1
port=${PORT:-8080}

(cd crates/app && dx build --platform web --release >/dev/null)
site=$(mktemp -d)
ln -s "$PWD/target/dx/app/release/web/public" "$site/conductor"
(cd crates/app/tests-e2e && [ -d node_modules ] || npm install --silent)
cargo build -q -p daemon --bin conductord

python3 -m http.server "$port" --bind 127.0.0.1 --directory "$site" >/dev/null 2>&1 &
server_pid=$!
host_log=$(mktemp)
data_dir=$(mktemp -d)
./target/debug/conductord s3-host --data-dir "$data_dir" --web-url "http://localhost:$port/conductor/" >"$host_log" 2>&1 &
host_pid=$!
trap 'kill $server_pid $host_pid 2>/dev/null || true; rm -rf "$site" "$host_log" "$data_dir"' EXIT
until grep -q '^join:' "$host_log"; do sleep 0.5; done
join=$(awk '/^join:/ {print $2}' "$host_log")

out=docs/s3/browser-run.txt
{
  echo "# Web build in headless Chromium and WebKit (390x844) against conductord s3-host, $(date -u +%FT%TZ)"
  echo
  wasm=$(ls -t target/dx/app/release/web/public/assets/*.wasm | head -1)
  echo "wasm: $(stat -c %s "$wasm") bytes raw, $(gzip -9 -c "$wasm" | wc -c) bytes gzip -9"
  echo
  echo "## Playwright, Chromium"
  ENGINE=chromium node crates/app/tests-e2e/browser.mjs "$join" /tmp/s3-chromium.png
  echo
  echo "## Playwright, WebKit"
  ENGINE=webkit node crates/app/tests-e2e/browser.mjs "$join" docs/s3/iphone-390x844.png
  echo
  echo "## Host log"
  sed -E 's/^[0-9TZ:.-]+ +//' "$host_log" | grep -v '^\(ticket\|join\|data dir\|qr image\):'
} >"$out" 2>&1 || { cat "$out"; exit 1; }
cat "$out"
