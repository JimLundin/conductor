#!/bin/sh
# The `codex` stand-in for codex_replay.rs, which writes fake-codex.py and
# the recording into its working directory.
exec /usr/bin/python3 -I fake-codex.py stdout.jsonl
