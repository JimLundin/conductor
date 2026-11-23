"""Builds the gated Codex recording the `codex-cli` plugin replay plays, from
the live `s2` recordings.

Run from anywhere: `python3 -I fixtures/e2e/build-codex.py`. The recording
is the `initialize` answer with the `model/list` answer, one `thread/start`
answer per session, then whole turns of `s2`'s thread, cut from one
`turn/start` answer to the next. The lines are Codex's own; only ids change,
and markers go in by rule:

- our requests are numbered in the order the hub sends them (`initialize`
  1, then each `thread/start` and `turn/start`), so every answer is
  renumbered in order and waits for our request with its id;
- Codex's own requests are renumbered from 0, as Codex numbers them per
  process, with the `requestId` of their `serverRequest/resolved`;
- a `serverRequest/resolved` for a request the session answers waits for that
  answer, with the decision `s2` gave.
"""

import json
import pathlib

HERE = pathlib.Path(__file__).parent
SOURCE = HERE.parent / "codex" / "server1-u5.jsonl"

# `s2`'s first two turns on its first thread: 1 approve, 2 decline.
APPROVE, DECLINE = range(2)


def wait(match):
    return json.dumps({"fake": "await", "match": match})


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
    """Waits for our decision before Codex resolves the request."""
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


def thread_of(start):
    return json.loads(start[0])["result"]["thread"]["id"]


before, parts = segments(SOURCE.read_text().splitlines())
head = [line for line in before if "userAgent" in line or '"data":' in line or "result" not in json.loads(line)]
threads = [p for p in parts if "thread" in json.loads(p[0])["result"]]
turns = [p for p in parts if "turn" in json.loads(p[0])["result"]]
first, second = threads[0], threads[1]
a, b = thread_of(first), thread_of(second)

approve_asked, approve_rest = until_asked(turns[APPROVE])
decline_asked, decline_rest = until_asked([line.replace(a, b) for line in turns[DECLINE]])

recording = (
    head
    + first
    + second
    + approve_asked
    + decline_asked
    + answered(approve_rest, "accept")
    + answered(decline_rest, "decline")
)
(HERE / "codex-two-threads-listed.jsonl").write_text("\n".join(renumbered(recording)) + "\n")
