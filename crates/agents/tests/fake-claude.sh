#!/bin/sh
# The `claude` stand-in: fake-cli.py plays stdout.jsonl from the working
# directory, gated on what the session writes.
exec /usr/bin/python3 -I "$(dirname "$0")/fake-cli.py"
