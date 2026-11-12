"""The `claude` and `codex` stand-in for the agents and daemon tests.

Plays the recording `stdout.jsonl` from the working directory and logs every
line it reads to `stdin.jsonl`. A line waits for what the session writes:

- A JSON-RPC response waits for our request with its id, and a
  `serverRequest/resolved` for our answer to that request, as Codex does.
- `{"fake": "await", "match": {...}}` waits for the next stdin line whose JSON
  has those keys and values. A nested object matches when the line's value
  there has its keys and values in turn, so `{"result": {}}` is any result.
  An `await` right before a `serverRequest/resolved` replaces its wait for
  our answer: Codex also resolves requests it withdrew, which nobody answers.
- `{"fake": "pace", "ms": 30}` sleeps that long before each later line.
- `{"fake": "release", "file": "go"}` waits until that file exists in the
  working directory. Later awaits match only lines read after it, so a test
  can hold the recording at a point and prove nothing was sent before.
- `{"fake": "hang_up", "after": 1}`, anywhere in the recording, closes stdin
  once that many lines are read, so the session's next write fails. The
  process stays up 1 s after the recording ends, so the failure isn't an exit.
- `{"fake": "exit"}` kills the process with SIGKILL, as a crash would.

Each start in the same directory is a new run. Run 1 plays `stdout.jsonl`
and logs `stdin.jsonl`; run n after it plays `stdout.n.jsonl`, or
`stdout.jsonl` again if there is none, and logs `stdin.n.jsonl`, so a test
can restart a CLI and have it resume.

Markers are never printed. Once stdin closes, nothing waits.

At start it records the environment variables its arguments name, those
set, as `NAME=value` lines in `env.txt`, and copies `$CODEX_HOME/config.toml`
to `codex-config.toml`, for tests of what a host passes its CLIs. A value
other than `1` without `conductor-test` in it is the environment's own,
maybe a real credential, so it is written as `<inherited>`.
"""

import json
import os
import signal
import sys
import threading
import time

run = int(open("runs.txt").read()) + 1 if os.path.exists("runs.txt") else 1
with open("runs.txt", "w") as runs:
    runs.write(str(run))
suffix = "" if run == 1 else f".{run}"

with open("env.txt", "w") as env:
    for name in sorted(n for n in sys.argv[1:] if n in os.environ):
        value = os.environ[name]
        env.write(f"{name}={value if 'conductor-test' in value or value == '1' else '<inherited>'}\n")
config = os.path.join(os.environ.get("CODEX_HOME", ""), "config.toml")
if "CODEX_HOME" in os.environ and os.path.exists(config):
    with open(config) as source, open("codex-config.toml", "w") as copy:
        copy.write(source.read())

played = f"stdout{suffix}.jsonl" if os.path.exists(f"stdout{suffix}.jsonl") else "stdout.jsonl"
recording = open(played).read().splitlines()
hang_up = next((m["after"] for m in map(json.loads, recording) if m.get("fake") == "hang_up"), None)
received = []
closed = False
changed = threading.Condition()


def read_stdin():
    global closed
    with open(f"stdin{suffix}.jsonl", "w") as log:
        for line in sys.stdin:
            log.write(line)
            log.flush()
            with changed:
                received.append(json.loads(line))
                changed.notify_all()
            if len(received) == hang_up:
                os.close(0)
                break
    with changed:
        closed = True
        changed.notify_all()


def wait_until(found, timeout=None):
    with changed:
        changed.wait_for(lambda: closed or found(), timeout)


def sent(id, is_request):
    return any(m.get("id") == id and ("method" in m) == is_request for m in received)


def has(want, got):
    if isinstance(want, dict):
        return isinstance(got, dict) and all(k in got and has(v, got[k]) for k, v in want.items())
    return want == got


def find(want, start):
    hits = (i for i in range(start, len(received)) if has(want, received[i]))
    return next(hits, None)


reader = threading.Thread(target=read_stdin)
reader.start()
matched = 0
pace = 0
awaited = False
for line in recording:
    line += "\n"
    msg = json.loads(line)
    if msg.get("fake") == "await":
        wait_until(lambda: find(msg["match"], matched) is not None)
        hit = find(msg["match"], matched)
        matched = len(received) if hit is None else hit + 1
        awaited = True
        continue
    if msg.get("fake") == "release":
        while not (closed or os.path.exists(msg["file"])):
            time.sleep(0.01)
        with changed:
            matched = len(received)
        continue
    if msg.get("fake") == "hang_up":
        continue
    if msg.get("fake") == "exit":
        os.kill(os.getpid(), signal.SIGKILL)
    if msg.get("fake") == "pace":
        pace = msg["ms"] / 1000
        continue
    if "id" in msg and "method" not in msg:
        wait_until(lambda: sent(msg["id"], is_request=True))
    elif msg.get("method") == "serverRequest/resolved" and not awaited:
        wait_until(lambda: sent(msg["params"]["requestId"], is_request=False))
    awaited = False
    time.sleep(pace)
    sys.stdout.write(line)
    sys.stdout.flush()
reader.join()
if hang_up is not None:
    time.sleep(1)
