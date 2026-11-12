#!/usr/bin/env bash
# Reprints the unit 5 review's measurements. Run from the repository root at
# the reviewed tip (cebf082): docs/subset/measurements/p1-u5/measure.sh
# Each required fix touches one file, so the per-file rows are per fix;
# mapping.rs is also split at `#[cfg(test)]` into its code and its tests.
set -euo pipefail
here=$PWD/docs/subset/measurements/p1-u5
cargo build -q -p subset
subset=$(realpath "$(tools/target-dir)")/debug/subset
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
files="crates/daemon/src/session.rs crates/daemon/src/codex_hub.rs
  crates/daemon/src/mapping.rs crates/daemon/tests/e2e.rs"

measure() {
  "$subset" score $files --by file
  sed '/^#\[cfg(test)\]/,$d' crates/daemon/src/mapping.rs >mapping_code.rs
  echo "mapping.rs without its tests:"
  "$subset" score mapping_code.rs | tail -1
  "$subset" score crates/daemon/src/mapping.rs --by fn |
    grep -E 'codex_thread_steps|codex_open|approval_tool'
}

git archive HEAD crates fixtures tools subset.toml | tar -x -C "$scratch"
cd "$scratch"
echo "# The tip"
measure
echo "# Unit 5's share: the same files on main"
git -C "$OLDPWD" archive main crates/daemon | tar -x -C "$scratch" --transform 's,^,main/,'
"$subset" score main/crates/daemon/src/session.rs main/crates/daemon/src/mapping.rs \
  main/crates/daemon/tests/e2e.rs --by file
sed '/^#\[cfg(test)\]/,$d' main/crates/daemon/src/mapping.rs >main_mapping_code.rs
"$subset" score main_mapping_code.rs | tail -1

echo "# With required-fixes.diff"
git apply "$here/required-fixes.diff"
measure
