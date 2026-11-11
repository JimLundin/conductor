#!/usr/bin/env bash
# Reprints every number in docs/subset/reviews/p1-u3.md. Run from the
# repository root of rv-p1-u3 with main (subset v6) merged in, after
# `cargo build -p subset`.
set -euo pipefail
root=$PWD
here=$root/docs/subset/measurements/p1-u3
subset=$root/target/debug/subset
base=087c1c2
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# config <name> <allowed id or ""> <tier or "">: a copy of subset.toml that
# also allows one id at one tier.
config() {
  local out=$work/$1.toml
  cp subset.toml "$out"
  if [ -n "$2" ]; then
    sed -i "s/^  \"pat.ident.mut\",.*/&\n  \"$2\",/" "$out"
    sed -i "s/^\"pat.wild\" = \"encouraged\"/&\n\"$2\" = \"$3\"/" "$out"
  fi
  echo "$out"
}
v6=$(config v6 "" "")
guard=$(config guard expr.match.guard tolerated)

# score <label> <config> <dir> <files...>: the total of the files, scored
# from inside <dir> so that `include` matches.
score() {
  local label=$1 cfg=$2 dir=$3
  shift 3
  printf '%-44s ' "$label"
  (cd "$dir" && "$subset" score "$@" --config "$cfg" | tail -1)
}

echo "== Volume: the unit against its base ($base)"
mkdir -p "$work/base"
git archive "$base" crates | tar -x -C "$work/base"
score "base: all of crates/" "$v6" "$work/base" $(cd "$work/base" && find crates -name '*.rs' | sort)
score "head: all of crates/" "$v6" "$root" $(find crates -name '*.rs' | sort)
score "base: transcript.rs" "$v6" "$work/base" crates/client/src/transcript.rs
score "head: host_view.rs" "$v6" "$root" crates/client/src/host_view.rs
score "base: host.rs + server.rs" "$v6" "$work/base" crates/daemon/src/{host,server}.rs
score "head: log.rs + registry.rs + serve.rs" "$v6" "$root" crates/daemon/src/{log,registry,serve}.rs
score "base: daemon lib.rs + main.rs" "$v6" "$work/base" crates/daemon/src/{lib,main}.rs
score "head: daemon lib.rs + main.rs" "$v6" "$root" crates/daemon/src/{lib,main}.rs
score "base: e2e.rs" "$v6" "$work/base" crates/daemon/tests/e2e.rs
score "head: e2e.rs" "$v6" "$root" crates/daemon/tests/e2e.rs
score "base: app" "$v6" "$work/base" crates/app/src/{main,ticket}.rs
score "head: app" "$v6" "$root" crates/app/src/{main,ticket}.rs
score "base: headless.rs" "$v6" "$work/base" crates/client/examples/headless.rs
score "head: headless.rs" "$v6" "$root" crates/client/examples/headless.rs
score "base: proto" "$v6" "$work/base" crates/proto/src/{lib,frame,ticket}.rs
score "head: proto" "$v6" "$root" crates/proto/src/{lib,frame,ticket}.rs

echo "== Required fixes, on the files they touch"
fixed=(crates/proto/src/ticket.rs crates/proto/src/frame.rs crates/daemon/src/serve.rs
  crates/daemon/src/main.rs crates/daemon/tests/e2e.rs)
mkdir -p "$work/fix"
git archive HEAD crates | tar -x -C "$work/fix"
score "branch tip" "$v6" "$work/fix" "${fixed[@]}"
(cd "$work/fix" && patch -s -p1 < "$here/required-fixes.diff")
score "with the required fixes" "$v6" "$work/fix" "${fixed[@]}"
for f in "${fixed[@]}"; do
  score "  tip   $f" "$v6" "$root" "$f"
  score "  fixed $f" "$v6" "$work/fix" "$f"
done

echo "== Ratchet: an equality guard (proposal 0040)"
echo "The after rows count each guard's comparison as outside (5.0); the"
echo "proposal subtracts 3.0 per comparison token to price it as tolerated."
mkdir -p "$work/r/crates/x/src"
cp "$here"/{changed,auth,feed,follow}-{before,after}.rs "$work/r/crates/x/src/"
for site in changed auth feed; do
  score "before $site" "$v6" "$work/r" "crates/x/src/$site-before.rs"
  score "after  $site" "$guard" "$work/r" "crates/x/src/$site-after.rs"
  (cd "$work/r" && "$subset" score "crates/x/src/$site-after.rs" --config "$guard" --by construct \
    | grep 'expr.binary.eq' || true)
done

echo "== Ratchet: a bare break in a drive loop (not proposed)"
echo "The after row counts the bare break as outside (5.0); costly is 3.0."
score "before follow" "$v6" "$work/r" crates/x/src/follow-before.rs
score "after  follow" "$v6" "$work/r" crates/x/src/follow-after.rs
