#!/usr/bin/env bash
# The live hosted check: a real `conductord host` running the real Claude CLI
# (model haiku), driven over iroh by the `live_host` example. It costs a few
# cents of API use, so it is not part of tools/check.sh.
#
#   tools/live-host.sh [log-dir]
#
# Prints the client's lines, then one RESULT line. Exits non-zero on a failed
# check. The host's own log is kept in log-dir (default: a temp dir).
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
target=$(tools/target-dir)
cargo build -q -p daemon --bin conductord
cargo build -q -p client --example live_host

scratch=/tmp/live-host-scratch
logs=${1:-$(mktemp -d)}
mkdir -p "$logs"
rm -rf "$scratch"
mkdir -p "$scratch"
data_dir=$(mktemp -d)
"$target"/debug/conductord host --data-dir "$data_dir" --claude-model haiku --no-webrtc \
  >"$logs/host.out" 2>"$logs/host.log" &
host_pid=$!
trap 'kill $host_pid 2>/dev/null || true; rm -rf "$data_dir"' EXIT
for _ in $(seq 120); do
  grep -q '^ticket:' "$logs/host.out" && break
  kill -0 $host_pid 2>/dev/null || { cat "$logs/host.log"; exit 1; }
  sleep 0.5
done
ticket=$(sed -n 's/^ticket: *//p' "$logs/host.out")

client=pass
"$target"/debug/examples/live_host "$ticket" --directory "$scratch" | tee "$logs/client.out" || client=fail

file=missing
[ -f "$scratch/a.txt" ] && file="present:$(tr -d '\n' <"$scratch/a.txt")"
case "$file" in present:hello) ;; *) client=fail ;; esac

# The third turn's interrupt is held when it reaches the session before the
# CLI's `system/init`, and goes straight through when init came first.
interrupt=direct
grep -q 'holding the interrupt until the turn starts' "$logs/host.log" && interrupt=held
if [ "$interrupt" = held ] && ! grep -q 'sending the held interrupt' "$logs/host.log"; then
  interrupt=held-but-never-sent
  client=fail
fi
echo "RESULT $client a.txt=$file interrupt-at-send=$interrupt logs=$logs"
[ "$client" = pass ]
