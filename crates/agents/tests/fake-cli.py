"""The `claude` and `codex` stand-in for the agents and daemon tests.

Plays the recording `stdout.jsonl` from the working directory and logs every
line it reads to `stdin.jsonl`. A line waits for what the session writes:

- A JSON-RPC response waits for our request with its id, and a
  `serverRequest/resolved` for our answer to that request, as Codex does.
- `{"fake": "await", "match": {...}}` waits for the next stdin line whose JSON
  has those keys and values.
- `{"fake": "pace", "ms": 30}` sleeps that long before each later line.
- `{"fake": "release", "file": "go"}` waits until that file exists in the
  working directory. Later awaits match only lines read after it, so a test
  can hold the recording at a point and prove nothing was sent before.

Markers are never printed. Once stdin closes, nothing waits.
"""

import json
import os
import sys
import threading
import time

received = []
closed = False
changed = threading.Condition()


def read_stdin():
    global closed
    with open("stdin.jsonl", "w") as log:
        for line in sys.stdin:
            log.write(line)
            log.flush()
            with changed:
                received.append(json.loads(line))
                changed.notify_all()
    with changed:
        closed = True
        changed.notify_all()


def wait_until(found, timeout=None):
    with changed:
        changed.wait_for(lambda: closed or found(), timeout)


def sent(id, is_request):
    return any(m.get("id") == id and ("method" in m) == is_request for m in received)


def find(want, start):
    hits = (i for i in range(start, len(received)) if all(received[i].get(k) == v for k, v in want.items()))
    return next(hits, None)


reader = threading.Thread(target=read_stdin)
reader.start()
matched = 0
pace = 0
for line in open("stdout.jsonl"):
    msg = json.loads(line)
    if msg.get("fake") == "await":
        wait_until(lambda: find(msg["match"], matched) is not None)
        hit = find(msg["match"], matched)
        matched = len(received) if hit is None else hit + 1
        continue
    if msg.get("fake") == "release":
        while not (closed or os.path.exists(msg["file"])):
            time.sleep(0.01)
        with changed:
            matched = len(received)
        continue
    if msg.get("fake") == "pace":
        pace = msg["ms"] / 1000
        continue
    if "id" in msg and "method" not in msg:
        wait_until(lambda: sent(msg["id"], is_request=True))
    elif msg.get("method") == "serverRequest/resolved":
        # Codex also resolves requests it withdrew, which nobody answers.
        wait_until(lambda: sent(msg["params"]["requestId"], is_request=False), timeout=5)
    time.sleep(pace)
    sys.stdout.write(line)
    sys.stdout.flush()
reader.join()
