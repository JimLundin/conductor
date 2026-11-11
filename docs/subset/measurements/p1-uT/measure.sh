#!/usr/bin/env bash
# Reprints every number in docs/subset/reviews/p1-uT.md. Run from the
# repository root of branch rv-p1-uT after `cargo build -p subset`.
set -euo pipefail
root=$PWD
here=$root/docs/subset/measurements/p1-uT
subset=$root/target/debug/subset
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

src=$work/crates/daemon/src
mkdir -p "$src"
cp crates/daemon/src/{wire,rtc,stun}.rs "$src/"

# config <name> <allowed id or ""> <tier or ""> [endpoint]: a copy of subset.toml
# that also allows one id at one tier, or lists one more endpoint.
config() {
  local out=$work/$1.toml
  cp subset.toml "$out"
  if [ -n "$2" ]; then
    sed -i "s/^  \"pat.ident.mut\",.*/&\n  \"$2\",/" "$out"
    sed -i "s/^\"pat.wild\" = \"encouraged\"/&\n\"$2\" = \"$3\"/" "$out"
  fi
  if [ -n "${4:-}" ]; then
    sed -i "s/^endpoints = \[/endpoints = [\"$4\", /" "$out"
  fi
  echo "$out"
}

score() {
  local label=$1 cfg=$2
  printf '%-28s ' "$label"
  (cd "$work" && "$subset" score "$src"/{wire,rtc,stun}.rs --config "$cfg" | tail -1)
  printf '%-28s ' ""
  "$subset" measure "$src"/{wire,rtc,stun}.rs | tail -1
}

v5=$(config v5 "" "")
score "branch tip" "$v5"

(cd "$work" && patch -s -p1 < "$here/required-fixes.diff")
score "with the required fixes" "$v5"
cp "$src/stun.rs" "$work/stun-fixed.rs"

cp "$here/stun-range.rs" "$src/stun.rs"
score "ranges in get(..)" "$(config range expr.range neutral)"
cp "$here/stun-slice.rs" "$src/stun.rs"
score "slice patterns" "$(config slice pat.slice encouraged)"
cp "$here/stun-reader.rs" "$src/stun.rs"
score "Reader endpoint" "$(config reader "" "" Reader)"
cp "$here/stun-buf.rs" "$src/stun.rs"
score "bytes::Buf (legal in v5)" "$v5"
