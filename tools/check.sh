#!/usr/bin/env bash
# The gate: the shared Rust standard, which runs Conductor's suites from
# checks.toml beside its own checks. It runs its units without a recorded
# pass. It exports STANDARDS for the web suites in checks.toml, which run
# the TypeScript standard on web/.
# `tools/check.sh [commit|step|release] [--no-cache|--verify]`;
# `--verify` runs nothing and fails each unit without a recorded pass.
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
		env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE 			git clone -q https://github.com/JimLundin/standards.git "$clone"
	[ -n "$(find "$clone/.git/FETCH_HEAD" -mmin -10 2>/dev/null)" ] ||
		env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE 			git -C "$clone" pull -q --ff-only || echo "warning: $clone not updated" >&2
fi
export STANDARDS=${STANDARDS:-$clone}
exec "$STANDARDS/rust/check.sh" "$@"
