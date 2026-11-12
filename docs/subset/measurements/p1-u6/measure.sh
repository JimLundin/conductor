#!/usr/bin/env bash
# Reprints the token counts in docs/subset/reviews/p1-u6.md. Applies each
# patch here, in order, to the reviewed commit in a throwaway worktree, and
# prints each file whose count changed.
# usage: docs/subset/measurements/p1-u6/measure.sh   (from the repository root)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(git rev-parse --show-toplevel)
cargo build -q -p subset --manifest-path "$root/Cargo.toml"
subset=$root/target/debug/subset
tree=$(mktemp -d)/tree
git -C "$root" worktree add -q --detach "$tree" 60be415
trap 'git -C "$root" worktree remove --force "$tree"' EXIT
files=(
  crates/app/src/main.rs crates/app/src/session.rs crates/app/src/sessions.rs
  crates/app/src/fixture.rs crates/app/src/ticket.rs
  crates/client/src/host_view.rs crates/client/src/new_session.rs
  crates/client/src/link.rs crates/proto/src/lib.rs
)
tokens() { [ -f "$tree/$1" ] && "$subset" measure "$tree/$1" | tail -1 | awk '{print $2}' || echo 0; }
declare -A before
for f in "${files[@]}"; do before[$f]=$(tokens "$f"); done
echo "screen files at 60be415: $((before[crates/app/src/main.rs] + before[crates/app/src/session.rs] + before[crates/app/src/sessions.rs]))"
for patch in "$here"/*.patch; do
  git -C "$tree" apply "$patch"
  echo "== $(basename "$patch" .patch)"
  total=0
  for f in "${files[@]}"; do
    after=$(tokens "$f")
    change=$((after - before[$f]))
    [ "$change" -ne 0 ] && printf '  %-34s %5s -> %5s (%+d)\n' "$f" "${before[$f]}" "$after" "$change"
    total=$((total + change)); before[$f]=$after
  done
  printf '  %-34s %+d\n' total "$total"
done
echo "screen files after all patches: $((before[crates/app/src/main.rs] + before[crates/app/src/session.rs] + before[crates/app/src/sessions.rs]))"
