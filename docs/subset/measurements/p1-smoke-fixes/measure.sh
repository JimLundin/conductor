#!/usr/bin/env bash
# Reprints every token count and score in docs/subset/reviews/p1-smoke-fixes.md.
# Run from the repository root at 3a61161 (rv-p1-smoke-fixes).
set -euo pipefail
here=docs/subset/measurements/p1-smoke-fixes
subset="$(tools/target-dir)/release/subset"
[ -x "$subset" ] || cargo build -q -p subset --release
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

files=(crates/app/src/main.rs crates/app/src/session.rs crates/app/src/sessions.rs
  crates/app/src/ticket.rs crates/client/src/device.rs crates/client/src/host_view.rs
  crates/proto/src/lib.rs crates/daemon/src/mapping.rs)
for v in main head fixed table; do
  mkdir -p "$work/$v"
  cp subset.toml "$work/$v/"
done
for f in "${files[@]}"; do
  for v in head fixed table; do
    mkdir -p "$work/$v/$(dirname "$f")"
    git show "3a61161:$f" > "$work/$v/$f"
  done
done
for f in crates/app/src/{main,session,sessions,ticket}.rs; do
  mkdir -p "$work/main/$(dirname "$f")"
  git show "main:$f" > "$work/main/$f"
done
(cd "$work/fixed" && patch -s -p1 < "$OLDPWD/$here/provider-note.patch" \
  && patch -s -p1 < "$OLDPWD/$here/no-create.patch" \
  && patch -s -p1 < "$OLDPWD/$here/working.patch")
(cd "$work/table" && patch -s -p1 < "$OLDPWD/$here/device-table.patch")

echo "== screen files (crates/app/src): main, branch, with the fixes"
for v in main head fixed; do (cd "$work/$v" && "$subset" measure crates/app/src/*.rs | tail -1); done

echo "== sessions.rs, branch then provider-note.patch"
for v in head fixed; do (cd "$work/$v" && "$subset" score crates/app/src/sessions.rs --by file | tail -1); done

echo "== no-create.patch over proto, session.rs, mapping.rs"
for v in head fixed; do (cd "$work/$v" && "$subset" measure crates/proto/src/lib.rs crates/app/src/session.rs crates/daemon/src/mapping.rs | tail -1); done

echo "== device.rs without its tests, branch then device-table.patch (0042)"
for v in head table; do
  python3 -I -c 'import sys; p=sys.argv[1]; s=open(p).read(); open(p,"w").write(s[:s.index("#[cfg(test)]")])' \
    "$work/$v/crates/client/src/device.rs"
  (cd "$work/$v" && "$subset" score crates/client/src/device.rs --by file | tail -1 \
    && "$subset" score crates/client/src/device.rs --by construct | grep type.lifetime || true)
done
echo "(0042's after score counts type.lifetime at tolerated, 2.0 a token, not outside, 5.0)"

echo "== device.rs's test module: branch, as a 'static Case table, as an inline tuple table"
mkdir -p "$work/tests/crates/client/src"
cp subset.toml "$work/tests/"
git show 3a61161:crates/client/src/device.rs \
  | python3 -I -c 'import sys; s=sys.stdin.read(); print(s[s.index("#[cfg(test)]"):], end="")' \
  > "$work/tests/crates/client/src/branch.rs"
cp "$here/device-test-static.rs" "$here/device-test-tuples.rs" "$work/tests/crates/client/src/"
(cd "$work/tests" && for f in branch device-test-static device-test-tuples; do
  "$subset" score "crates/client/src/$f.rs" --by file | tail -1
done)
