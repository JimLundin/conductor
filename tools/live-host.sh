#!/usr/bin/env bash
# The live hosted check: a real `conductord host` running the real Claude CLI
# (model haiku) or Codex CLI (model gpt-6-luna), driven over iroh by three
# runs of the `headless` example. It costs a few cents of API use, so it is
# not part of tools/check.sh.
#
#   tools/live-host.sh [--provider claude|codex] [--register] [--data-dir DIR] [log-dir]
#
# The host runs the pinned CLIs in its private homes, so it needs a
# credential or a sign-in there. --register sends the credentials this
# shell uses with SetCredential, before the first turn: Claude's gateway
# from ANTHROPIC_BASE_URL and ANTHROPIC_AUTH_TOKEN, or the Codex provider
# that ~/.codex/config.toml selects, with its key from the variable its
# env_key names. Nothing of them is printed. Otherwise run `conductord
# login claude|codex --data-dir DIR` once and pass the same --data-dir,
# which is kept. Without --data-dir, a scratch dir under the target dir is
# used and removed; not under /tmp, where Codex refuses to create its
# helper binaries in CODEX_HOME.
#
# Prints one RESULT line. Exits non-zero on a failed check. Each run's output
# and the host's own log are kept in log-dir (default: a temp dir).
set -euo pipefail
cd "$(dirname "$0")/.."
provider=claude
data_dir=
register=keep
while [ $# -gt 0 ]; do
  case $1 in
    --provider) provider=$2; shift 2 ;;
    --register) register=env; shift ;;
    --data-dir) data_dir=$2; shift 2 ;;
    *) break ;;
  esac
done
case $provider in
  claude) create="Use the Write tool to create a.txt in the current directory containing exactly the word hello. Then say done." ;;
  # Codex writes in the workspace without asking, so the prompt asks it to
  # escalate, which needs an approval.
  codex) create="Create a.txt in the current directory containing exactly the word hello, with one shell command run with escalated permissions, so that I approve it first. Then say done." ;;
  *) echo "unknown provider: $provider" >&2; exit 2 ;;
esac
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
target=$(tools/target-dir)
cargo build -q -p daemon --bin conductord
cargo build -q -p client --example headless

scratch=/tmp/live-host-scratch
logs=${1:-$(mktemp -d)}
mkdir -p "$logs"
rm -rf "$scratch"
mkdir -p "$scratch"
cleanup=
if [ -z "$data_dir" ]; then
  data_dir=$(mktemp -d "$target/live-host.XXXXXX")
  cleanup=$data_dir
fi
# A registered Codex provider brings its own model.
models=(--claude-model haiku --codex-model gpt-6-luna)
[ "$register" = keep ] || models=(--claude-model haiku)
"$target"/debug/conductord host --data-dir "$data_dir" "${models[@]}" \
  >"$logs/host.out" 2>"$logs/host.log" &
host_pid=$!
# SIGINT lets the host stop its CLIs, which keep writing in the data dir.
trap 'kill -INT $host_pid 2>/dev/null || true; wait $host_pid 2>/dev/null || true; [ -z "$cleanup" ] || rm -rf "$cleanup"' EXIT
# The first start in a data dir downloads the pinned CLIs.
for _ in $(seq 600); do
  grep -q '^ticket:' "$logs/host.out" && break
  kill -0 $host_pid 2>/dev/null || { cat "$logs/host.log"; exit 1; }
  sleep 0.5
done
ticket=$(sed -n 's/^ticket: *//p' "$logs/host.out")

# Only the headless runs see the Codex provider's values, never the host.
codex_provider() {
  python3 -I - <<'EOF'
import os, shlex, tomllib
config = tomllib.load(open(os.path.expanduser("~/.codex/config.toml"), "rb"))
provider = config["model_providers"][config["model_provider"]]
assert provider.get("wire_api", "responses") == "responses", "only the responses wire API"
values = {
    "NAME": provider.get("name", config["model_provider"]),
    "BASE_URL": provider["base_url"],
    "MODEL": config["model"],
    "KEY": os.environ[provider["env_key"]],
}
for name, value in values.items():
    print(f"export CONDUCTOR_CODEX_{name}={shlex.quote(value)}")
EOF
}
case $provider$register in codexenv) eval "$(codex_provider)" ;; esac

count="Count from 1 to 300, one number per line, with no other text."
turn() { # name, expected outcome, headless args...
  local name=$1 outcome=$2; shift 2
  "$target"/debug/examples/headless "$ticket" --provider "$provider" --directory "$scratch" "$@" >"$logs/$name.out" || true
  grep -v '(history)' "$logs/$name.out" | grep -q "TurnCompleted { outcome: $outcome }" && echo pass || echo fail
}
approve=$(turn approve Completed --prompt "$create" --register "$register")
grep -q 'registered the credential -> gateway' "$logs/approve.out" || [ "$register" = keep ] || approve=fail
grep -q 'Approve -> Accepted' "$logs/approve.out" || approve=fail
delta=$(turn first-delta Interrupted --prompt "$count" --interrupt first-delta)
at_send=$(turn at-send Interrupted --prompt "$count" --interrupt at-send)
grep -q 'interrupt -> Accepted' "$logs/at-send.out" || at_send=fail

file=missing
[ -f "$scratch/a.txt" ] && file="present:$(tr -d '\n' <"$scratch/a.txt")"
case "$file" in present:hello) ;; *) approve=fail ;; esac
interrupt=direct
grep -q 'holding the interrupt until the turn starts' "$logs/host.log" && interrupt=held
grep -q 'holding the interrupt' "$logs/host.log" && ! grep -q 'sending the held interrupt' "$logs/host.log" && at_send=fail
echo "RESULT provider=$provider register=$register approve=$approve first-delta=$delta at-send=$at_send a.txt=$file interrupt-at-send=$interrupt logs=$logs"
[ "$approve$delta$at_send" = passpasspass ]
