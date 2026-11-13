#!/usr/bin/env bash
# Phone-sized checkpoint diff / middle-turn undo / reconnect checks. Outputs
# remain in target; Chromium and WebKit are emulation, not physical iPhone tests.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH" NO_COLOR=1
port=${PORT:-8081}
target=$(tools/target-dir)
run="$target/issue-13-browser"
mkdir -p "$run"
(cd crates/app && dx build --platform web --release >"$run/build-web.txt" 2>&1)
cargo build -p daemon --bin conductord --example worktrees_browser >"$run/build-host.txt" 2>&1
(cd crates/app/tests-e2e && [ -d node_modules ] || npm install --silent)
site=$(mktemp -d)
ln -s "$target/dx/app/release/web/public" "$site/conductor"
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$site" >"$run/server.txt" 2>&1 &
server=$!
hosts=()
cleanup() {
  kill "$server" "${hosts[@]}" 2>/dev/null || true
  rm -rf "$site"
}
trap cleanup EXIT
for engine in chromium webkit; do
  scratch=$(mktemp -d "$run/$engine.XXXXXX")
  source="$scratch/source"
  data="$scratch/host"
  mkdir -p "$source" "$data/probe"
  git -C "$source" init -q
  echo 'baseline' >"$source/baseline.txt"
  git -C "$source" add baseline.txt
  git -C "$source" -c user.name=Conductor -c user.email=conductor@example.invalid commit -qm baseline -m 'Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>'
  cp fixtures/e2e/claude-worktrees.jsonl "$source/stdout.jsonl"
  cp fixtures/e2e/claude-worktrees-fork.jsonl "$source/stdout.2.jsonl"
  head -2 fixtures/e2e/claude-killed.jsonl >"$data/probe/stdout.jsonl"
  "$target/debug/conductord" host --data-dir "$data" \
    --web-url "http://localhost:$port/conductor/" \
    --claude-path "$PWD/crates/agents/tests/fake-claude.sh" \
    --codex-path "$scratch/no-codex" >"$scratch/host.txt" 2>&1 &
  hosts+=($!)
  until rg -q '^join:' "$scratch/host.txt"; do
    kill -0 "${hosts[-1]}" 2>/dev/null || { cat "$scratch/host.txt"; exit 1; }
    sleep 0.5
  done
  ticket=$(awk '/^ticket:/ {print $2}' "$scratch/host.txt")
  join=$(awk '/^join:/ {print $2}' "$scratch/host.txt")
  "$target/debug/examples/worktrees_browser" "$ticket" "$source" >"$scratch/seed.txt" 2>&1
  ENGINE="$engine" node crates/app/tests-e2e/browser.mjs "$join" "$run/$engine.png" worktrees unused >"$run/$engine.txt" 2>&1
  echo "$engine checkpoint/undo browser checks passed: $run/$engine.txt"
done
