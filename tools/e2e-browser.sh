#!/usr/bin/env bash
# Browser end to end: builds the web app, serves it under /conductor/ like
# GitHub Pages does, and drives headless Chromium and WebKit at an iPhone
# viewport against `conductord host` running the fake CLIs
# (crates/agents/tests/fake-*.sh) through n0's relays. Each scenario gets its
# own host: a Claude turn with approval in both engines (turn), two Codex
# sessions on one app-server (codex, Chromium only), and the screenshots of
# the states a real CLI shows only with careful timing (screens).
# Output: docs/p1/browser-run.txt and docs/p1/screens/*-{running,closed,idle}.png.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH" NO_COLOR=1
port=${PORT:-8080}
recordings=$PWD/fixtures/e2e
fakes=$PWD/crates/agents/tests

target=$(tools/target-dir)
(cd crates/app && dx build --platform web --release >/dev/null)
site=$(mktemp -d)
ln -s "$target/dx/app/release/web/public" "$site/conductor"
(cd crates/app/tests-e2e && [ -d node_modules ] || npm install --silent)
cargo build -q -p daemon --bin conductord

python3 -m http.server "$port" --bind 127.0.0.1 --directory "$site" >/dev/null 2>&1 &
server=$!
# Fixed session paths, which the screenshots show, so a rerun that changes
# nothing redraws nothing. Two runs at once would share it.
scratch=/tmp/conductor-e2e
rm -rf "$scratch"
mkdir -p "$scratch"
trap 'kill "$server" 2>/dev/null || true; rm -rf "$site" "$scratch"' EXIT

# session <dir> <recording> [filter...]: a directory holding the recording,
# passed through the filter, as the fake's stdout.jsonl.
session() {
  local dir=$1 recording=$2
  shift 2
  mkdir -p "$dir"
  "${@:-cat}" <"$recordings/$recording" >"$dir/stdout.jsonl"
}
# Claude's turn plays a line every 200 ms once the message arrives, so the
# reply streams.
paced() { sed '/"match": {"type": "user"}/a {"fake": "pace", "ms": 200}'; }
# Paced, and held after the reply's third delta for a `release` file that
# never comes.
held() { paced | awk '{print} /"text_delta"/ && ++n == 3 {print "{\"fake\": \"release\", \"file\": \"release\"}"}'; }

# start_host <name> [flags...]: starts a host on the fake CLIs with its own
# data dir and sets $join to its join link. Its log is $scratch/<name>.log.
# Codex is unavailable unless $codex names its CLI.
start_host() {
  local name=$1
  shift
  mkdir -p "$scratch/$name"
  "$target"/debug/conductord host --data-dir "$scratch/$name" \
    --web-url "http://localhost:$port/conductor/" \
    --claude-path "$fakes/fake-claude.sh" --codex-path "${codex:-$scratch/no-codex}" "$@" \
    >"$scratch/$name.log" 2>&1 &
  hosts+=($!)
  until grep -q '^join:' "$scratch/$name.log"; do
    kill -0 "${hosts[-1]}" 2>/dev/null || { cat "$scratch/$name.log"; return 1; }
    sleep 0.5
  done
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

# run <engine> <scenario> <output> <session dir>
run() {
  ENGINE=$1 node crates/app/tests-e2e/browser.mjs "$join" "$3" "$2" "$4"
}

# turn_run <engine> <scenario> <output>: a scenario's Claude turn, in a
# session directory named after the run.
turn_run() {
  session "$scratch/$2-$1" claude-approve.jsonl paced
  run "$1" "$2" "$3" "$scratch/$2-$1"
}

out=docs/p1/browser-run.txt
set +e
(
  set -e
  hosts=()
  trap 'kill "${hosts[@]}" 2>/dev/null || true' EXIT
  echo "# Web build in headless Chromium and WebKit (390x844) against conductord host on the fake CLIs, $(date -u +%FT%TZ)"
  echo
  wasm=$(ls -t "$target"/dx/app/release/web/public/assets/*.wasm | head -1)
  echo "wasm: $(stat -c %s "$wasm") bytes raw, $(gzip -9 -c "$wasm" | wc -c) bytes gzip -9"

  start_host turn
  echo
  echo "## Turn, Chromium"
  turn_run chromium turn "$scratch/chromium.png"
  echo
  echo "## Turn, WebKit"
  turn_run webkit turn "$scratch/webkit.png"
  sleep 2
  echo
  echo "## Turn, host log"
  host_log turn
  echo
  expect turn 6 'serve: command client="iPhone"' 'both turns'"'"' commands reached the host'

  session "$scratch/codex" codex-two-threads.jsonl
  mkdir "$scratch/codex/first" "$scratch/codex/second"
  codex=$fakes/fake-codex.sh start_host codex
  echo
  echo "## Codex, two sessions on one app-server, chromium"
  run chromium codex "$scratch/codex.png" "$scratch/codex"
  echo
  echo "## Codex, host log"
  host_log codex
  echo
  expect codex 1 'version=fake-codex' 'the host found Codex'

  start_host credentials
  echo
  echo "## Credentials, webkit"
  run webkit credentials "$scratch/credentials.png" "$scratch"
  echo
  echo "## Credentials, host log"
  host_log credentials
  echo
  expect credentials 2 'stored a credential change' 'the gateway was stored, then cleared'
  expect credentials 0 'conductor-test-browser-token' 'the token is in no log line'

  # conductor runs, held mid-reply; scratch's CLI hangs up as the session
  # starts; notes is never sent a message.
  session "$scratch/screens/conductor" claude-approve.jsonl held
  session "$scratch/screens/scratch" claude-hang-up.jsonl
  session "$scratch/screens/notes" claude-approve.jsonl
  start_host screens
  echo
  echo "## Screens, chromium"
  run chromium screens docs/p1/screens "$scratch/screens"
) >"$out" 2>&1
status=$?
python3 tools/scrub-ips.py "$out"
set -e
cat "$out"
exit $status
