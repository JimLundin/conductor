"""Builds the gated Claude recordings the `claude-cli` replay tests play, from
the live `s1` recordings.

Run from anywhere: `python3 -I fixtures/e2e/build-claude.py`. Markers go in
by rule, so every gate matches what a session writes:

- the `initialize` response waits for a `control_request`;
- each turn's `system/init` waits for a user message;
- a `can_use_tool` request is followed by a wait for our `control_response`;
- any later `control_response` (an interrupt's ack) waits for a `control_request`.

Acks are renumbered `req_1`, `req_2`, ... in order, because the session that
replays a recording numbers its control requests from 1.
"""

import json
import pathlib
import re

HERE = pathlib.Path(__file__).parent
SESSION1 = HERE.parent / "claude" / "session1-approve-deny-interrupt.jsonl"

# The turns of `s1` session 1 by step: 1 approve, 2 deny, 3 interrupt at
# start, pong, 3c interrupt mid-stream, pong, 3d interrupt at send, pong,
# 3b interrupt with a permission open.
RECORDINGS = {
    "claude-approve.jsonl": (SESSION1, [0]),
    "claude-deny-interrupt.jsonl": (SESSION1, [1, 4, 5]),
}


def wait(match):
    return json.dumps({"fake": "await", "match": match})


def permission_response(msg, recording):
    body = msg["request"]
    verdict = {"behavior": "allow", "updatedInput": body["input"]}
    if recording == "claude-deny-interrupt.jsonl":
        verdict = {"behavior": "deny"}
    return {
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": msg["request_id"],
            "response": verdict,
        },
    }


def gated(line, recording=None):
    msg = json.loads(line)
    kind = (msg.get("type"), msg.get("subtype"))
    if kind == ("control_response", None):
        return [wait({"type": "control_request"}), line]
    if kind == ("system", "init"):
        return [wait({"type": "user"}), line]
    if kind == ("control_request", None) and msg["request"]["subtype"] == "can_use_tool":
        return [line, wait(permission_response(msg, recording))]
    return [line]


def turns(lines):
    """The initialize response, then each turn from its `init` to its `result`."""
    starts = [i for i, line in enumerate(lines) if '"subtype":"init"' in line]
    ends = [i + 1 for i, line in enumerate(lines) if json.loads(line).get("type") == "result"]
    return lines[0], [lines[s:e] for s, e in zip(starts, ends)]


def renumbered(lines):
    acks = iter(range(1, len(lines) + 1))
    ack = re.compile(r'^(\{"type":"control_response","response":\{"subtype":"success","request_id":")req_\d+"')
    return [ack.sub(lambda m: f'{m.group(1)}req_{next(acks)}"', line) for line in lines]


for name, (source, picked) in RECORDINGS.items():
    initialize, played = turns(source.read_text().splitlines())
    lines = renumbered([initialize] + [line for t in picked for line in played[t]])
    turn = [m for line in lines[1:] for m in gated(line, name)]
    out = gated(lines[0]) + turn
    (HERE / name).write_text("\n".join(out) + "\n")
