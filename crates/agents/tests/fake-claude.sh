#!/bin/sh
# The `claude` stand-in: fake-cli.py plays stdout.jsonl from the working
# directory, gated on what the session writes.
# `--version` answers as the real CLI does, so a host finds it available.
[ "$1" = --version ] && { echo "fake-claude"; exit 0; }
# Each run appends its arguments to argv.txt, one per line, after a `run`
# line, for tests of what a host passes its CLI.
{ echo run; printf '%s\n' "$@"; } >> argv.txt
# The arguments name the variables it records (fake-cli.py).
exec /usr/bin/python3 -I "$(dirname "$0")/fake-cli.py" \
  ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN ANTHROPIC_BASE_URL CLAUDE_CODE_OAUTH_TOKEN CLAUDE_CODE_SUBPROCESS_ENV_SCRUB CLAUDE_CONFIG_DIR
