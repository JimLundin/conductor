#!/usr/bin/env bash
# The gate: the shared Rust standard, which runs Conductor's suites from
# checks.toml beside its own checks, then the shared TypeScript standard on
# the browser suite in crates/app/tests-e2e. Each runs its units without a
# recorded pass, and the gate fails if either fails. `tools/check.sh
# [commit|step|release] [--no-cache]`.
#
# The standards come from a clone of the standards project on the Linux
# filesystem (`STANDARDS_CLONE`, default ~/projects/tools/standards), which
# runs the checker several times faster than the Windows checkout through
# /mnt/c.
# The clone fast-forwards at most every ten minutes, so pushed standards
# reach the gate without a fetch on every run. `STANDARDS` overrides it,
# e.g. to try unpushed standards from the Windows checkout.
set -uo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
clone=${STANDARDS_CLONE:-$HOME/projects/tools/standards}
if [ -z "${STANDARDS:-}" ]; then
	[ -d "$clone/.git" ] ||
		git clone -q https://github.com/JimLundin/standards.git "$clone"
	[ -n "$(find "$clone/.git/FETCH_HEAD" -mmin -10 2>/dev/null)" ] ||
		git -C "$clone" pull -q --ff-only || echo "warning: $clone not updated" >&2
fi
standards=${STANDARDS:-$clone}
status=0
"$standards/rust/check.sh" "$@" || status=1
source tools/e2e-common.sh
target=$(tools/target-dir) || exit 1
lock_browser || exit 1
install_browser_dependencies || exit 1
"$standards/typescript/check.sh" "$@" crates/app/tests-e2e || status=1
exit "$status"
