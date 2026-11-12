#!/bin/sh
# The `codex` stand-in: fake-cli.py plays stdout.jsonl from the working
# directory, gated on what the server writes.
# `--version` answers as the real CLI does, so a host finds it available.
[ "$1" = --version ] && { echo "fake-codex"; exit 0; }
# The arguments name the variables it records (fake-cli.py).
exec /usr/bin/python3 -I "$(dirname "$0")/fake-cli.py" CODEX_HOME CONDUCTOR_CODEX_KEY
