"""Builds the gated Codex e2e recordings from the live `s2` recording.

Run from anywhere: `python3 -I fixtures/e2e/build-codex.py`. Each recording
is the `initialize` answer, one `thread/start` answer per session, then whole
turns of `s2`'s thread, cut from one `turn/start` answer to the next. The
lines are Codex's own; only ids change, and markers go in by rule:

- our requests are numbered in the order the hub sends them (`initialize`
  1, then each `thread/start`, `turn/start` and `turn/interrupt`), so every
  answer is renumbered in order and waits for our request with its id;
- Codex's own requests are renumbered from 0, as Codex numbers them per
  process, with the `requestId` of their `serverRequest/resolved`;
- a `serverRequest/resolved` for a request the session answers waits for that
  answer, with the decision `s2` gave;
- an interrupt step waits for our `turn/interrupt`: after the first delta
  (mid-stream), on `turn/started` once the test's `release` file exists
  (held), or before Codex withdraws the open request (withdrawn).
"""

import json
import pathlib

HERE = pathlib.Path(__file__).parent
SOURCE = HERE.parent / "codex" / "server1-u5.jsonl"

# `s2`'s turns on its first thread, by step: 1 approve, 2 decline, 3
# interrupt at start, pong, 3c interrupt mid-stream, pong, 3d interrupt at
# send, pong, 3b interrupt with an approval open.
APPROVE, DECLINE, AT_START, _, MID_STREAM, PONG, _, _, WITHDRAW = range(9)


def wait(match):
    return json.dumps({"fake": "await", "match": match})


def interrupt(turn=None):
    params = {} if turn is None else {"params": {"turnId": turn}}
    return wait({"method": "turn/interrupt", **params})


def segments(lines):
    """The `initialize` answer, the thread starts and the turns, each from its
    answer up to the next answer that starts a thread or a turn."""
    starts = [
        i
        for i, line in enumerate(lines)
        if {"thread", "turn"} & set(json.loads(line).get("result") or {})
    ]
    cuts = starts + [len(lines)]
    return lines[: starts[0]], [lines[s:e] for s, e in zip(cuts, cuts[1:])]


def answered(turn, decision):
    """Waits for our answer before Codex resolves the request."""
    out = []
    for line in turn:
        msg = json.loads(line)
        if msg.get("method") == "serverRequest/resolved":
            out.append(wait({"id": msg["params"]["requestId"], "result": {"decision": decision}}))
        out.append(line)
    return out


def until_asked(turn):
    """The turn up to and including its approval request, and the rest."""
    asked = next(i for i, line in enumerate(turn) if 'requestApproval"' in line)
    return turn[: asked + 1], turn[asked + 1 :]


def mid_stream(turn):
    first = next(i for i, line in enumerate(turn) if '"item/agentMessage/delta"' in line)
    return turn[: first + 1] + [interrupt()] + turn[first + 1 :]


def held(turn):
    started = next(i for i, line in enumerate(turn) if '"turn/started"' in line)
    turn_id = json.loads(turn[started])["params"]["turn"]["id"]
    release = json.dumps({"fake": "release", "file": "release"})
    return turn[:started] + [release, turn[started], interrupt(turn_id)] + turn[started + 1 :]


def withdrawn(turn):
    resolved = next(i for i, line in enumerate(turn) if '"serverRequest/resolved"' in line)
    return turn[:resolved] + [interrupt()] + turn[resolved:]


def renumbered(lines):
    """Our requests' answers from 1 and Codex's requests from 0, in order."""
    ours = iter(range(1, len(lines) + 1))
    theirs = {}
    out = []
    for line in lines:
        msg = json.loads(line)
        if msg.get("fake") == "await" and "id" in msg["match"]:
            msg["match"]["id"] = theirs[msg["match"]["id"]]
        elif "fake" in msg:
            pass
        elif "id" in msg and "method" not in msg:
            msg["id"] = next(ours)
        elif "id" in msg:
            msg["id"] = theirs.setdefault(msg["id"], len(theirs))
        elif msg.get("method") == "serverRequest/resolved":
            msg["params"]["requestId"] = theirs[msg["params"]["requestId"]]
        else:
            out.append(line)
            continue
        out.append(json.dumps(msg, separators=(",", ":")))
    return out


def on_thread(lines, old, new):
    return [line.replace(old, new) for line in lines]


def thread_of(start):
    return json.loads(start[0])["result"]["thread"]["id"]


before, parts = segments(SOURCE.read_text().splitlines())
# The hub asks for no model list, so only the `initialize` answer stays.
head = [line for line in before if "userAgent" in line or "result" not in json.loads(line)]
threads = [p for p in parts if "thread" in json.loads(p[0])["result"]]
turns = [p for p in parts if "turn" in json.loads(p[0])["result"]]
first, second = threads[0], threads[1]
a, b = thread_of(first), thread_of(second)

approve_asked, approve_rest = until_asked(turns[APPROVE])
decline_asked, decline_rest = until_asked(on_thread(turns[DECLINE], a, b))

RECORDINGS = {
    "codex-two-threads.jsonl": head
    + first
    + second
    + approve_asked
    + decline_asked
    + answered(approve_rest, "accept")
    + answered(decline_rest, "decline"),
    "codex-interrupt.jsonl": head + first + mid_stream(turns[MID_STREAM]) + turns[PONG],
    "codex-held-interrupt.jsonl": head + first + held(turns[AT_START]),
    "codex-withdraw.jsonl": head + first + withdrawn(turns[WITHDRAW]),
    # The stand-in stops reading once both threads have started, then exits.
    "codex-exit.jsonl": [json.dumps({"fake": "hang_up", "after": 4})] + head + first + second,
}

for name, lines in RECORDINGS.items():
    (HERE / name).write_text("\n".join(renumbered(lines)) + "\n")
