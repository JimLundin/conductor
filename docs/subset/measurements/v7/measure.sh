#!/usr/bin/env bash
# Reprints the version 7 rewrite's measurements: every file that moved to
# 0040's guard or 0041's `for`, including the review's fixes. Before is main at 3b7b30c (unit 6 done)
# scored by its own tool and subset.toml (version 6); after is this tree
# under version 7. Run from the repository root:
# docs/subset/measurements/v7/measure.sh
set -euo pipefail
before=3b7b30c
cargo build -q -p subset
subset=$(realpath "$(tools/target-dir)")/debug/subset
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
git archive "$before" | tar -x -C "$scratch"
(cd "$scratch" && cargo build -q -p subset --target-dir "$scratch/target")
files="crates/daemon/src/session.rs crates/daemon/src/serve.rs crates/daemon/tests/e2e.rs
  crates/agents/tests/codex_replay.rs crates/agents/src/bin/s1.rs crates/agents/src/bin/s2.rs
  crates/client/src/host_view.rs crates/client/src/link.rs
  crates/daemon/src/rtc.rs crates/agents/src/claude/session.rs crates/agents/src/codex/server.rs
  crates/client/src/platform/rtc.rs crates/client/examples/headless.rs"

echo "# Before: $before under version 6"
(cd "$scratch" && target/debug/subset score $files --by file)
echo "# Before: $before under version 7 (the replaced forms score as outside)"
cp subset.toml "$scratch/subset-v7.toml"
(cd "$scratch" && "$subset" score --config subset-v7.toml $files --by file)
echo "# After: this tree under version 7"
"$subset" score $files --by file
