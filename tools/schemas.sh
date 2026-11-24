#!/usr/bin/env bash
# Regenerates web/src/proto/ from link: the JSON Schema its test writes,
# then the Effect Schemas web/scripts/schemas.ts prints from it, both in
# Biome's format. `tools/schemas.sh [--check]`; with --check, the
# `schemas` suite, it fails when that changed what is committed.
# Needs web/node_modules (`npm ci` in web/).
set -euo pipefail
cd "$(dirname "$0")/.."
standards=${STANDARDS:-$HOME/projects/tools/standards}
biome=$(sed -n 's/^BIOME=//p' "$standards/tools.env")
cargo test --quiet -p link --lib schema::
(
	cd web
	node scripts/schemas.ts
	npx -y "@biomejs/biome@$biome" format --write src/proto >/dev/null
)
if [ "${1:-}" = --check ]; then
	stale=$(
		git diff --name-only -- web/src/proto
		git ls-files --others --exclude-standard -- web/src/proto
	)
	if [ -n "$stale" ]; then
		printf '%s\n' "$stale"
		echo "web/src/proto/ is stale: run tools/schemas.sh and commit it" >&2
		exit 1
	fi
	echo "web/src/proto/ matches link"
fi
