#!/bin/sh
# The `codex` stand-in: fake-cli.py plays stdout.jsonl from the working
# directory, gated on what the server writes.
exec /usr/bin/python3 -I "$(dirname "$0")/fake-cli.py"
