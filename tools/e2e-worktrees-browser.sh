#!/usr/bin/env bash
# Phone-sized checkpoint diff / middle-turn undo / reconnect checks. Outputs
# remain in target; Chromium and WebKit are emulation, not physical iPhone tests.
set -euo pipefail
umask 077
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH" NO_COLOR=1
source tools/e2e-common.sh
target=$(tools/target-dir)
lock_browser
install_browser_dependencies
init_browser_scratch
run="$target/issue-13-browser"
mkdir -p "$run"
(cd crates/app && dx build --platform web --release >"$run/build-web.txt" 2>&1)
cargo build -p conductord --bin conductord --example worktrees_browser >"$run/build-host.txt" 2>&1
start_browser_server
real_git=$(command -v git)
mkdir -p "$scratch/bin"
cat >"$scratch/bin/git" <<'SH'
#!/usr/bin/env bash
if [[ " $* " == *" diff --no-ext-diff --no-textconv --no-renames --name-status "* ]] &&
   rmdir "$PREVIEW_FAILURE" 2>/dev/null; then
  echo 'fixture checkpoint preview failure' >&2
  exit 1
fi
exec "$REAL_GIT" "$@"
SH
chmod +x "$scratch/bin/git"
hosts=()
trap 'stop_children "${hosts[@]}"; cleanup_browser' EXIT
for engine in chromium webkit; do
  work=$(mktemp -d "$scratch/$engine.XXXXXX")
  source="$work/source"
  data="$work/host"
  mkdir -p "$source" "$data/work/claude" "$work/fail-preview"
  git -C "$source" init -q
  echo 'baseline' >"$source/baseline.txt"
  git -C "$source" add baseline.txt
  git -C "$source" -c user.name=Conductor -c user.email=conductor@example.invalid commit -qm baseline -m 'Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>'
  cp fixtures/e2e/claude-worktrees.jsonl "$source/stdout.jsonl"
  cp fixtures/e2e/claude-worktrees-fork.jsonl "$source/stdout.2.jsonl"
  head -2 fixtures/e2e/claude-killed.jsonl >"$data/work/claude/stdout.jsonl"
  REAL_GIT="$real_git" PREVIEW_FAILURE="$work/fail-preview" PATH="$scratch/bin:$PATH" \
    "$target/debug/conductord" host --data-dir "$data" \
    --web-url "http://localhost:$port/conductor/" \
    --cli claude="$PWD/crates/claude-cli/tests/fake-claude.sh" \
    --cli codex="$work/no-codex" >"$work/host.txt" 2>&1 9>&- &
  hosts+=($!)
  wait_for_join "${hosts[-1]}" "$engine" "$work/host.txt"
  ticket=$(awk '/^ticket:/ {print $2}' "$work/host.txt")
  join=$(awk '/^join:/ {print $2}' "$work/host.txt")
  "$target/debug/examples/worktrees_browser" "$ticket" "$source" >"$work/seed.txt" 2>&1
  ENGINE="$engine" node crates/app/tests-e2e/browser.ts "$join" "$run/$engine.png" worktrees unused >"$run/$engine.txt" 2>&1
  echo "$engine checkpoint/undo browser checks passed: $run/$engine.txt"
done
