#!/usr/bin/env bash
# Reprints the unit 4 review's measurements. Run from the repository root at
# the reviewed tip (1d1cabe): docs/subset/measurements/p1-u4/measure.sh
set -euo pipefail
here=$PWD/docs/subset/measurements/p1-u4
cargo build -q -p subset
subset=$(realpath "$(tools/target-dir)")/debug/subset
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
git archive HEAD crates fixtures tools subset.toml | tar -x -C "$scratch"
cd "$scratch"
files="crates/client/examples/headless.rs crates/client/examples/live_host.rs
  crates/daemon/src/mapping.rs crates/daemon/tests/e2e.rs crates/daemon/src/session.rs"

echo "# The tip"
"$subset" score $files --by file

echo "# With required-fixes.diff (fixes 1 to 5)"
git apply "$here/required-fixes.diff"
"$subset" score $(ls $files 2>/dev/null) --by file
"$subset" score crates/daemon/src/mapping.rs --by fn | grep -E 'each_claude'

echo "# Fix 6, with f2-checker.diff: next_input inlined into run"
git apply "$here/f2-checker.diff"
python3 - <<'PY'
p = "crates/daemon/src/session.rs"
s = open(p).read()
old = """            let input = self.next_input(&mut inbox).await;
            self.on(input).await?;
        }
    }

    async fn next_input(&mut self, inbox: &mut mpsc::Receiver<SessionMsg>) -> Input {
        tokio::select! {
            msg = inbox.recv() => msg.map_or(Input::Closed, Input::Order),
            input = self.feed.next() => input,
        }
    }
"""
new = """            let input = tokio::select! {
                msg = inbox.recv() => msg.map_or(Input::Closed, Input::Order),
                input = self.feed.next() => input,
            };
            self.on(input).await?;
        }
    }
"""
assert old in s
open(p, "w").write(s.replace(old, new))
PY
"$subset" score crates/daemon/src/session.rs --by file

echo "# Proposal 0041: for .. await in loop state, expr.for tolerated"
sed -e 's/^  "expr.loop",                # proposal 0007$/&\n  "expr.for",/' \
    -e 's/^"expr.loop" = "costly"                # 0007$/&\n"expr.for" = "tolerated"/' \
    subset.toml >subset-0041.toml
git -C "$OLDPWD" show HEAD:crates/daemon/src/session.rs >crates/daemon/src/session.rs
"$subset" score --config subset-0041.toml crates/daemon/src/session.rs --by fn |
  grep -E 'apply|perform|settle|withdraw|effect|total'
cp "$here/f1-after.rs" crates/daemon/src/session.rs
"$subset" score --config subset-0041.toml crates/daemon/src/session.rs --by fn |
  grep -E 'apply|step$|withdraw|total'
