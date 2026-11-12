#!/bin/sh
# The `claude` stand-in: fake-cli.py plays stdout.jsonl from the working
# directory, gated on what the session writes.
# `--version` answers as the real CLI does, so a host finds it available.
[ "$1" = --version ] && { echo "fake-claude"; exit 0; }
exec /usr/bin/python3 -I "$(dirname "$0")/fake-cli.py"
