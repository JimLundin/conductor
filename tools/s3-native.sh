#!/usr/bin/env bash
# Native end to end through n0's relays: starts `conductord s3-host`, runs the
# headless client once straight through and once with a dropped connection
# mid-turn, and writes both sides' transcripts to docs/s3/native-run.txt.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH" NO_COLOR=1
cargo build -q -p daemon --bin conductord
cargo build -q -p client --example headless
out=docs/s3/native-run.txt
host_log=$(mktemp)
data_dir=$(mktemp -d)
target=$(tools/target-dir)
"$target"/debug/conductord s3-host --data-dir "$data_dir" >"$host_log" 2>&1 &
host_pid=$!
trap 'kill $host_pid 2>/dev/null || true; rm -rf "$host_log" "$data_dir"' EXIT
until grep -q '^ticket:' "$host_log"; do sleep 0.5; done
ticket=$(awk '/^ticket:/ {print $2}' "$host_log")
{
  echo "# conductord s3-host and the headless client, $(date -u +%FT%TZ)"
  echo
  echo "## Client run 1: prompt, deltas, approve, finish"
  "$target"/debug/examples/headless "$ticket" --prompt "what is the answer?"
  echo
  echo "## Client run 2: the connection drops after 4 deltas, then resumes"
  "$target"/debug/examples/headless "$ticket" --prompt "and again?" --reconnect-after 4
  echo
  echo "## Host log"
  sed -E 's/^[0-9TZ:.-]+ +//' "$host_log" | grep -v '^data dir:'
} >"$out" 2>&1
cat "$out"
