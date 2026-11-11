#!/bin/sh
# The `claude` stand-in for replay.rs: prints the recording in its working
# directory, then logs what the session writes to its stdin.
cat stdout.jsonl
cat >stdin.jsonl
