#!/usr/bin/env bash
# Browser end to end: builds the web app, serves it under /conductor/ like
# GitHub Pages does, and drives headless Chromium and WebKit at an iPhone
# viewport against `conductord s3-host`, first through n0's relays, then over
# the WebRTC upgrade. Three scenarios, each against its own host: the upgrade,
# a forced upgrade failure (fallback), and WebRTC dropped mid-turn (drop).
# Output: docs/s3/browser-run.txt and docs/s3/iphone-390x844.png (from WebKit).
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH" NO_COLOR=1
port=${PORT:-8080}
drop_after=8

(cd crates/app && dx build --platform web --release >/dev/null)
site=$(mktemp -d)
ln -s "$PWD/target/dx/app/release/web/public" "$site/conductor"
(cd crates/app/tests-e2e && [ -d node_modules ] || npm install --silent)
cargo build -q -p daemon --bin conductord

python3 -m http.server "$port" --bind 127.0.0.1 --directory "$site" >/dev/null 2>&1 &
pids=($!)
scratch=$(mktemp -d)
trap 'kill "${pids[@]}" 2>/dev/null || true; rm -rf "$site" "$scratch"' EXIT

# start_host <name> [flags...]: starts a host with its own data dir and sets
# $join to its join link. Its log is $scratch/<name>.log.
start_host() {
  local name=$1
  shift
  mkdir "$scratch/$name"
  ./target/debug/conductord host --data-dir "$scratch/$name" \
    --web-url "http://localhost:$port/conductor/" "$@" >"$scratch/$name.log" 2>&1 &
  pids+=($!)
  until grep -q '^join:' "$scratch/$name.log"; do sleep 0.5; done
  join=$(awk '/^join:/ {print $2}' "$scratch/$name.log")
}

# host_log <name>: the host's log without the ticket, QR code and paths.
host_log() {
  sed -E 's/^[0-9TZ:.-]+ +//' "$scratch/$1.log" |
    grep -v '^\(ticket\|join\|data dir\|qr image\|host id\|relay\):' | grep -v '[█▀▄]'
}

# expect <name> <count> <pattern> <what>: the host log has <count> lines matching.
expect() {
  local got
  got=$(host_log "$1" | grep -c -- "$3" || true)
  [ "$got" -eq "$2" ] || { echo "FAILED: host $1: $4 (expected $2, got $got)"; return 1; }
  echo "ok: host $1: $4 ($got)"
}

run() {
  local engine=$1 scenario=$2 shot=$3
  ENGINE=$engine node crates/app/tests-e2e/browser.mjs "$join" "$shot" "$scenario"
}

out=docs/s3/browser-run.txt
set +e
(
  set -e
  echo "# Web build in headless Chromium and WebKit (390x844) against conductord s3-host, $(date -u +%FT%TZ)"
  echo
  wasm=$(ls -t target/dx/app/release/web/public/assets/*.wasm | head -1)
  echo "wasm: $(stat -c %s "$wasm") bytes raw, $(gzip -9 -c "$wasm" | wc -c) bytes gzip -9"

  start_host upgrade
  echo
  echo "## Upgrade, Chromium"
  run chromium upgrade "$scratch/chromium.png"
  echo
  echo "## Upgrade, WebKit"
  run webkit upgrade docs/s3/iphone-390x844.png
  sleep 2
  echo
  echo "## Upgrade, host log"
  host_log upgrade
  echo
  expect upgrade 4 'command .*transport="webrtc"' 'both turns'"'"' commands went over WebRTC'
  expect upgrade 0 'command .*transport="iroh"' 'no command went over iroh'

  for engine in chromium webkit; do
    start_host "fallback-$engine" --test-rtc-blackhole
    echo
    echo "## Fallback (host drops every WebRTC datagram), $engine"
    run "$engine" fallback "$scratch/fallback-$engine.png"
    sleep 1
    echo
    echo "## Fallback, $engine, host log"
    host_log "fallback-$engine"
    echo
    expect "fallback-$engine" 2 'command .*transport="iroh"' 'the turn'"'"'s commands went over iroh'
    expect "fallback-$engine" 0 'webrtc connected' 'WebRTC never connected'
  done

  for engine in chromium webkit; do
    start_host "drop-$engine" --test-rtc-drop-after "$drop_after"
    echo
    echo "## Drop (host drops WebRTC after $drop_after frames, mid-turn), $engine"
    run "$engine" drop "$scratch/drop-$engine.png"
    sleep 2
    echo
    echo "## Drop, $engine, host log"
    host_log "drop-$engine"
    echo
    expect "drop-$engine" 1 'test knob: dropping the connection' 'WebRTC was dropped once'
    expect "drop-$engine" 1 'command .*Prompt.*transport="webrtc"\|command .*transport="webrtc".*Prompt' 'the prompt went over WebRTC'
    expect "drop-$engine" 1 'subscribed transport="iroh" after_seq=[1-9]' 'the client resubscribed on iroh from its cursor'
    expect "drop-$engine" 2 'subscribed transport="webrtc"' 'the client subscribed over WebRTC before the drop and again after'
  done
) >"$out" 2>&1
status=$?
set -e
cat "$out"
exit $status
