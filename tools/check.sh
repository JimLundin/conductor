#!/usr/bin/env bash
# The gate: the shared Rust standard, which runs Conductor's suites from
# checks.toml beside its own checks. `tools/check.sh [commit|step|release]
# [--base <ref>|--all] [--no-cache]`.
#
# The standards come from a clone of the home repository on the Linux
# filesystem (`STANDARDS_HOME`, default ~/projects/tools/home), which runs
# the checker several times faster than the Windows checkout through /mnt/c.
# The clone fast-forwards at most every ten minutes, so pushed standards
# reach the gate without a fetch on every run. `STANDARDS` overrides it,
# e.g. to try unpushed standards from the Windows checkout.
set -uo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
home=${STANDARDS_HOME:-$HOME/projects/tools/home}
if [ -z "${STANDARDS:-}" ]; then
	[ -d "$home/.git" ] || git clone -q https://github.com/JimLundin/home.git "$home"
	[ -n "$(find "$home/.git/FETCH_HEAD" -mmin -10 2>/dev/null)" ] ||
		git -C "$home" pull -q --ff-only || echo "warning: $home not updated" >&2
fi
exec "${STANDARDS:-$home/standards}/rust/check.sh" "$@"
