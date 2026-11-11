"""Builds the gated Claude e2e recordings from the live `s1` recording.

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
SOURCE = HERE.parent / "claude" / "session1-approve-deny-interrupt.jsonl"

# The turns of `s1` session 1 by step: 1 approve, 2 deny, 3 interrupt at
# start, pong, 3c interrupt mid-stream, pong, 3d interrupt at send, pong,
# 3b interrupt with a permission open.
RECORDINGS = {
    "claude-approve.jsonl": [0],
    "claude-deny-interrupt.jsonl": [1, 4, 5],
}


def wait(match):
    return json.dumps({"fake": "await", "match": match})


def gated(line):
    msg = json.loads(line)
    kind = (msg.get("type"), msg.get("subtype"))
    if kind == ("control_response", None):
        return [wait({"type": "control_request"}), line]
    if kind == ("system", "init"):
        return [wait({"type": "user"}), line]
    if kind == ("control_request", None) and msg["request"]["subtype"] == "can_use_tool":
        return [line, wait({"type": "control_response"})]
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


initialize, played = turns(SOURCE.read_text().splitlines())
for name, picked in RECORDINGS.items():
    lines = renumbered([initialize] + [line for t in picked for line in played[t]])
    out = [m for line in lines for m in gated(line)]
    (HERE / name).write_text("\n".join(out) + "\n")
