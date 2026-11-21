#!/usr/bin/env bash
# The live hosted check: a real `conductord host` running the real Claude CLI
# (model haiku) or Codex CLI (model gpt-6-luna), driven over iroh by five
# runs of the `headless` example. The last kills the host's CLI between two
# turns and continues the session. It costs a few cents of API use, so it is
# not part of tools/check.sh. The crash scenario uses Linux pidfds.
#
#   tools/live-host.sh [--provider claude|codex] [--register] [--model MODEL] [--undo] [--data-dir DIR] [log-dir]
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
# --model requests a model listed by the chosen instance for every session.
# --undo adds two edit/codeword turns, reads the second diff, undoes that
# turn, and checks restored files plus retained conversation history.
#
# Prints one RESULT line. Exits non-zero on a failed check. Each run's output
# and the host's own log are kept in log-dir (default: a temp dir).
set -euo pipefail
cd "$(dirname "$0")/.."
provider=claude
data_dir=
register=keep
undo_check=no
session_model=
while [ $# -gt 0 ]; do
  case $1 in
    --provider) provider=$2; shift 2 ;;
    --register) register=env; shift ;;
    --model) session_model=$2; shift 2 ;;
    --undo) undo_check=yes; shift ;;
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
python3 -c 'import os, signal; os.close(os.pidfd_open(os.getpid())); assert hasattr(signal, "pidfd_send_signal")' 2>/dev/null || {
  echo "the live-host crash check requires Linux pidfd support" >&2
  exit 2
}
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
target=$(tools/target-dir)
cargo build -q -p conductord
cargo build -q -p net --example headless

mkdir -p "$target"
scratch=$(mktemp -d "$target/live-host-scratch.XXXXXX")
logs=${1:-$(mktemp -d "$target/live-host-logs.XXXXXX")}
mkdir -p "$logs"
git -C "$scratch" init -q
printf 'source stays untouched\n' > "$scratch/source.txt"
git -C "$scratch" add source.txt
git -C "$scratch" -c user.name=Conductor -c user.email=conductor@example.invalid \
  commit -q -m "Live check baseline" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
source_index=$(git -C "$scratch" hash-object .git/index)
cleanup=
if [ -z "$data_dir" ]; then
  data_dir=$(mktemp -d "$target/live-host.XXXXXX")
  cleanup=$data_dir
fi
host_models=()
session_model_args=()
if [ -n "$session_model" ]; then
  session_model_args=(--model "$session_model")
else
  # Defaults still work when the provider's discovery omits these aliases.
  host_models=(--model claude=haiku --model codex=gpt-6-luna)
  [ "$register" = keep ] || host_models=(--model claude=haiku)
fi
"$target"/debug/conductord host --data-dir "$data_dir" "${host_models[@]}" \
  >"$logs/host.out" 2>"$logs/host.log" &
host_pid=$!
# SIGINT lets the host stop its CLIs, which keep writing in the data dir.
trap 'kill -INT $host_pid 2>/dev/null || true; wait $host_pid 2>/dev/null || true; [ -z "$cleanup" ] || { rm -rf "$cleanup"; rm -rf "$scratch"; }' EXIT
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
account_args=()
case $provider$register in
  claudeenv) account_args=(--account "$ANTHROPIC_BASE_URL") ;;
  codexenv)
    eval "$(codex_provider)"
    account_args=(--account "$CONDUCTOR_CODEX_BASE_URL")
    ;;
esac
headless_args=("$ticket" --provider "$provider" --directory "$scratch" "${account_args[@]}" "${session_model_args[@]}")

selected=own
if [ "$register" = env ]; then
  selected=gateway
fi

count="Count from 1 to 300, one number per line, with no other text."
turn() { # name, expected outcome, headless args...
  local name=$1 outcome=$2; shift 2
  "$target"/debug/examples/headless "${headless_args[@]}" "$@" >"$logs/$name.out" || true
  grep -q "^.*] confirmed session instance $selected$" "$logs/$name.out" &&
    grep -v '(history)' "$logs/$name.out" | grep -q "TurnCompleted { outcome: $outcome }" && echo pass || echo fail
}
approve=$(turn approve Completed --prompt "$create" --register "$register")
grep -q 'registered the credential -> gateway' "$logs/approve.out" || [ "$register" = keep ] || approve=fail
grep -q 'Approve -> Accepted' "$logs/approve.out" || approve=fail
delta=$(turn first-delta Interrupted --prompt "$count" --interrupt first-delta)
at_send=$(turn at-send Interrupted --prompt "$count" --interrupt at-send)
grep -q 'interrupt -> Accepted' "$logs/at-send.out" || at_send=fail

# The leak canary: a command the agent runs counts, in its own environment,
# the variable that carries the secret. It prints counts only, never a
# value. PATH must count 1, which shows the command ran in a shell at all.
# Codex always keeps its key out. Claude does only where its sandbox runs
# (bwrap and socat on PATH); without it the token reaches commands, which
# the host warns about, and the canary reports leak=unprotected.
case $provider in
  claude) leak_vars=ANTHROPIC_AUTH_TOKEN ;;
  codex) leak_vars=CONDUCTOR_CODEX_KEY ;;
esac
leak_cmd="for v in PATH $leak_vars; do env | grep -c \"^\$v=\"; done | tr '\\n' ' ' > leak.txt"
leak=$(turn leak Completed --prompt "Run this exact shell command in the current directory, unchanged, then say done: $leak_cmd")
workspace() { sed -n 's/^.*] workspace //p' "$1" | tail -n 1; }
leak_workspace=$(workspace "$logs/leak.out")
leak_counts=$(sed 's/ *$//' "$leak_workspace/leak.txt" 2>/dev/null || echo missing)
case $provider:$leak:$leak_counts in
  *:pass:"1 0") ;;
  claude:pass:"1 1")
    grep -q "agent: the CLI's commands can read the gateway token;.*provider=Claude" "$logs/host.log" && leak=unprotected || leak=fail ;;
  *) leak=fail ;;
esac

# Continue: a turn, then the host's CLI killed as a crash would, then the
# session continued and asked what the first turn said.
continue_out=$logs/continue.out
"$target"/debug/examples/headless "${headless_args[@]}" \
  --timeout-secs 300 --prompt "My codeword is quince. Reply with just the word ok, and use no tools." \
  --then-continue "What is my codeword? Answer with just the word, and use no tools." >"$continue_out" &
continue_pid=$!
for _ in $(seq 480); do
  grep -q 'waiting for the agent to exit' "$continue_out" && break
  kill -0 $continue_pid 2>/dev/null || break
  sleep 0.5
done
crash=fail
if grep -q 'waiting for the agent to exit' "$continue_out" &&
  grep -q "^.*] confirmed session instance $selected$" "$continue_out" &&
  python3 tools/live-host-child.py "$host_pid" "$data_dir" "$provider" "$(workspace "$continue_out")"; then
  crash=pass
else
  kill "$continue_pid" 2>/dev/null || true
fi
wait $continue_pid || true
continued=fail
[ "$crash" = pass ] &&
  grep -q 'continue after .* -> Accepted' "$continue_out" &&
  grep -v '(history)' "$continue_out" | grep -qi 'AssistantMessage { text: "quince' &&
  grep -q 'turn completed$' "$continue_out" && continued=pass

approve_workspace=$(workspace "$logs/approve.out")
file=missing
[ -f "$approve_workspace/a.txt" ] && file="present:$(tr -d '\n' <"$approve_workspace/a.txt")"
case "$file" in present:hello) ;; *) approve=fail ;; esac
undo=skipped
if [ "$undo_check" = yes ]; then
  undo=$(turn undo Completed --timeout-secs 300 \
    --prompt "My retained codeword is amber. Create undo.txt containing exactly retained, in the current directory. Then reply done." \
    --then-undo "My discarded codeword is cobalt. Replace undo.txt with exactly discarded and create discarded-only.txt containing exactly discarded, in the current directory. Then reply done.")
  grep -q 'undo verified retained history' "$logs/undo.out" || undo=fail
  grep -v '(history)' "$logs/undo.out" | grep -q 'TurnsUndone' || undo=fail
  undo_workspace=$(workspace "$logs/undo.out")
  [ "$(cat "$undo_workspace/undo.txt" 2>/dev/null)" = retained ] || undo=fail
  [ ! -e "$undo_workspace/discarded-only.txt" ] || undo=fail
fi
source=pass
[ -z "$(git -C "$scratch" status --porcelain)" ] || source=fail
[ "$(git -C "$scratch" hash-object .git/index)" = "$source_index" ] || source=fail
interrupt=direct
grep -q 'holding the interrupt until the turn starts' "$logs/host.log" && interrupt=held
grep -q 'holding the interrupt' "$logs/host.log" && ! grep -q 'sending the held interrupt' "$logs/host.log" && at_send=fail
echo "RESULT provider=$provider register=$register instance=$selected approve=$approve first-delta=$delta at-send=$at_send continue=$continued leak=$leak leak-counts=${leak_counts// /,} undo=$undo source=$source source-repo=$scratch a.txt=$file interrupt-at-send=$interrupt logs=$logs"
case $approve$delta$at_send$continued:$leak in passpasspasspass:pass | passpasspasspass:unprotected) ;; *) exit 1 ;; esac

case $undo:$source in skipped:pass | pass:pass) ;; *) exit 1 ;; esac
