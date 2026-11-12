"""Builds the gated Claude e2e recordings from the live `s1` recordings.

Run from anywhere: `python3 -I fixtures/e2e/build-claude.py`. Markers go in
by rule, so every gate matches what a session writes:

- the `initialize` response waits for a `control_request`;
- each turn's `system/init` waits for a user message, and in the recordings
  listed in `RELEASED` also for the test's `release` file;
- a `can_use_tool` request is followed by a wait for our `control_response`;
- any later `control_response` (an interrupt's ack) waits for a `control_request`;
- in the recordings listed in `WITHDRAWN`, nothing waits for an answer to
  `can_use_tool`, and the `control_cancel_request` waits for the interrupt
  in place of the ack after it.

Acks are renumbered `req_1`, `req_2`, ... in order, because the session that
replays a recording numbers its control requests from 1.
"""

import json
import pathlib
import re

HERE = pathlib.Path(__file__).parent
SESSION1 = HERE.parent / "claude" / "session1-approve-deny-interrupt.jsonl"
SESSION3 = HERE.parent / "claude" / "session3-questions.jsonl"
PROBE = HERE.parent / "claude" / "probe-thinking.jsonl"
SESSION4 = HERE.parent / "claude" / "session4-chosen-killed.jsonl"
SESSION5 = HERE.parent / "claude" / "session5-resumed.jsonl"
SESSION6 = HERE.parent / "claude" / "session6-resume-missing.jsonl"

# The turns of `s1` session 1 by step: 1 approve, 2 deny, 3 interrupt at
# start, pong, 3c interrupt mid-stream, pong, 3d interrupt at send, pong,
# 3b interrupt with a permission open. Session 3: 7 a question, 8 a command
# approved for the session, the same command again with no request, 9 a
# question withdrawn by an interrupt. The probe: one turn with summarized
# thinking. Session 4: 12 a turn that reasons (its thinking empty, through
# the owner's gateway); session 5: 13 the same session resumed.
RECORDINGS = {
    "claude-approve.jsonl": (SESSION1, [0]),
    "claude-deny-interrupt.jsonl": (SESSION1, [1, 4, 5]),
    "claude-held-interrupt.jsonl": (SESSION1, [2]),
    "claude-hang-up.jsonl": (SESSION1, []),
    "claude-withdraw.jsonl": (SESSION1, [8]),
    "claude-question.jsonl": (SESSION3, [0]),
    "claude-for-session.jsonl": (SESSION3, [1, 2]),
    "claude-question-withdraw.jsonl": (SESSION3, [3]),
    "claude-reasoning.jsonl": (PROBE, [0]),
    "claude-killed.jsonl": (SESSION4, [0]),
    "claude-resumed.jsonl": (SESSION5, [0]),
}

# After its turns, the stand-in reads the next message and dies, as a CLI
# killed mid-turn does.
KILLED = {"claude-killed.jsonl"}

# The turn starts only once the test has sent its interrupt, so the session
# must hold it until `init` (docs/architecture.md §5.1).
RELEASED = {"claude-held-interrupt.jsonl"}

# The stand-in stops reading after `initialize`, so the first message fails.
HUNG_UP = {"claude-hang-up.jsonl"}

# The test never answers the request. Its interrupt makes the CLI withdraw it.
WITHDRAWN = {"claude-withdraw.jsonl", "claude-question-withdraw.jsonl"}


def wait(match):
    return json.dumps({"fake": "await", "match": match})


def gated(line, released, withdrawn):
    msg = json.loads(line)
    kind = (msg.get("type"), msg.get("subtype"))
    if kind == ("control_cancel_request", None):
        return [wait({"type": "control_request"}), line]
    if kind == ("control_response", None):
        return [line] if withdrawn else [wait({"type": "control_request"}), line]
    if kind == ("system", "init"):
        release = [json.dumps({"fake": "release", "file": "release"})] if released else []
        return [wait({"type": "user"})] + release + [line]
    if kind == ("control_request", None) and msg["request"]["subtype"] == "can_use_tool":
        return [line] if withdrawn else [line, wait({"type": "control_response"})]
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
    hang_up = [json.dumps({"fake": "hang_up", "after": 1})] if name in HUNG_UP else []
    turn = [m for line in lines[1:] for m in gated(line, name in RELEASED, name in WITHDRAWN)]
    killed = [wait({"type": "user"}), json.dumps({"fake": "exit"})] if name in KILLED else []
    out = hang_up + gated(lines[0], False, False) + turn + killed
    (HERE / name).write_text("\n".join(out) + "\n")

# A `--resume` whose transcript is gone: the CLI prints its one `result`,
# writes the same reason to stderr and exits 1, before `initialize`.
refused = SESSION6.read_text().splitlines()
reason = json.loads(refused[-1])["errors"][-1]
(HERE / "claude-resume-missing.jsonl").write_text(
    "\n".join(refused + [json.dumps({"fake": "fail", "stderr": reason, "status": 1})]) + "\n"
)
