#!/usr/bin/env bash
# Regenerates the client link's schemas: the JSON Schema link's test
# publishes as crates/link/link.schema.json, then the Effect Schemas
# web/scripts/schemas.ts prints from it into web/src/schemas/, in Biome's
# format. `tools/schemas.sh [--check]`; with --check, the
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
	npx -y "@biomejs/biome@$biome" format --write src/schemas >/dev/null
)
if [ "${1:-}" = --check ]; then
	stale=$(
		git diff --name-only -- crates/link/link.schema.json web/src/schemas
		git ls-files --others --exclude-standard -- crates/link/link.schema.json web/src/schemas
	)
	if [ -n "$stale" ]; then
		printf '%s\n' "$stale"
		echo "the client link's schemas are stale: run tools/schemas.sh and commit them" >&2
		exit 1
	fi
	echo "the client link's schemas match link"
fi
