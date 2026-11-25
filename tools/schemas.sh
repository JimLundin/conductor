#!/usr/bin/env bash
# Regenerates the client link's schemas: the JSON Schema link's `schema`
# example prints, into crates/link/link.schema.json, then the Effect Schemas
# web/scripts/schemas.ts prints from it into web/src/schemas/, both in
# the standard's oxfmt format. `tools/schemas.sh [--check]`; with --check, the `schemas`
# suite, it fails when that changed what is committed, or when the document
# differs from main's and link::ALPN doesn't.
# Needs web/node_modules (`npm ci` in web/).
set -euo pipefail
cd "$(dirname "$0")/.."
standards=${STANDARDS:-$HOME/projects/tools/standards}
oxfmt=$(sed -n 's/^OXFMT=//p' "$standards/tools.env")
document=crates/link/link.schema.json
cargo run --quiet -p link --example schema >"$document"
(
	cd web
	node scripts/schemas.ts
	npx -y "oxfmt@$oxfmt" --config "$standards/typescript/.oxfmtrc.json" src/schemas "../$document" >/dev/null
)
if [ "${1:-}" = --check ]; then
	stale=$(
		git diff --name-only -- "$document" web/src/schemas
		git ls-files --others --exclude-standard -- "$document" web/src/schemas
	)
	if [ -n "$stale" ]; then
		printf '%s\n' "$stale"
		echo "the client link's schemas are stale: run tools/schemas.sh and commit them" >&2
		exit 1
	fi
	alpn='^pub const ALPN'
	if ! git diff --quiet origin/main -- "$document" &&
		[ "$(git show origin/main:crates/link/src/lib.rs | grep "$alpn")" = "$(grep "$alpn" crates/link/src/lib.rs)" ]; then
		echo "the client link's schema differs from main's: bump link::ALPN" >&2
		exit 1
	fi
	echo "the client link's schemas match link"
fi
