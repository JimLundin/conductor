# Review: deterministic drift checks (drift-checks)

This is the reviewer's pass over `a390d38` (`git diff 43d047b...rv-drift`, three commits), under subset version 5. There is no friction log for this task. The live evidence is the ten logs in `/tmp/drift-runs/` (`s1` and `s2`, five runs each, all steps PASS) and one raw `s1` recording in `/tmp/s1-fix-try/session1.jsonl`, made at 14:31 before the five runs. The scratch file for the one measurement is `docs/subset/measurements/drift-checks/s1-merged-deny.rs`.

The verdict is **accept the API change, with three required fixes**. One is in the code, and two are in the protocol record. The bigger cost is not in this diff. Step 3 no longer interrupts a turn mid-stream, and P1 unit 2 planned to build its interrupt recording from exactly that step (see "Impact on the P1 design").

## Compliance

`subset check` reports nothing under `crates/agents/`. All 335 workspace violations are in `app` and `daemon`, the same as on `main`. `tools/check.sh` passes on `a390d38` (fmt, clippy native and wasm, 746 tests, 0 failed).

The diff holds six of the seven qualities. It breaches the seventh.

- **Named states.** `Plan::InterruptOnStart` names the new plan. Deleting `News` loses no state, because dropping the deduplication removes the decision `News` named.
- **Visible branching.** The new interrupt is the same tolerated `if let (Plan::…, None)` one-case branch that the old code used (0027), moved from `on_reply` to the `TurnStarted` arm. `on_reply` no longer awaits, so it is now a plain `fn`. That is correct under 0006.
- **Forward data flow, Errors are values, Local reasoning, One way.** Kept. `Actor::track` now returns `()` and only updates `open_permissions`. It is easier to read than the version that also decided which events to forward.
- **Repetition is data: breached.** `bin/s1.rs:212-218` adds a `(Plan::InterruptOnStart, _)` arm. It has the same shape as the `(Plan::InterruptOnPermission, Some(_))` arm above it, and only the message string differs (`"interrupted"` and `"late request"`). Both arms answer a permission request that should not arrive after an interrupt.

### Required fixes

1. **Merge the two deny arms in `s1.rs` `on_permission`** into `(Plan::InterruptOnPermission, Some(_)) | (Plan::InterruptOnStart, _)` with one message, such as `"not expected after an interrupt"`. Measured with the scratch copy, this takes `s1.rs` from 2537.5 to 2500.5 (-37.0) and from 450 to 443 lines. That is 21 points below the pre-change file.
2. **Print the turn's text in the detail line of `s1` step 3**, as `s2` already does (`output before the interrupt landed {:?}`). `claude/PROTOCOL.md` says the CLI "prints no assistant message … (Step 3, 2026-11-11, five runs.)". The five logs cannot show that, because the step 3 line prints `permissions` and `finished` but not `t.text`. `run.sh` passed no fixture directory, so the CLI's stdout was not recorded either. The only raw evidence is the single 14:31 recording, and it does agree (`init`, `control_response`, `[Request interrupted by user]`, `result aborted_streaming`, with no `assistant` frame). Either rerun with the text printed, or change the citation to "one recorded run".
3. **Fix the stale citation in `claude/PROTOCOL.md`.** The "Interrupt during streaming" entry still cites "(Step 3.)", but step 3 no longer interrupts during streaming. Retag it to the committed recording (`fixtures/claude/session1-approve-deny-interrupt.jsonl`, 2026-11-10), and state that no live step checks it now. Also, `tests/replay.rs:271` still labels that fixture "3: interrupt right after the first assistant output". That label is correct for the recording, so keep it. The fixture now records the old step 3, not the current one, and the PROTOCOL header should say so.

### Advisory

- `s2` does not isolate the user's Codex config. The new PROTOCOL note says Codex runs `~/.codex/hooks.json` before it answers `turn/interrupt`. So the drift check's timing depends on the owner's hooks, which `s1` avoids with `--setting-sources=project,local`. If `ThreadOptions`/`ServerOptions` can point `CODEX_HOME` at a scratch directory, do that. Until then, the note is an observation about this machine, not about Codex.
- `s1` step 3's `InterruptOnStart` deny path never runs, because the prompt asks for no tools. That is fine as a guard, and fix 1 makes it cost nothing.

### Volume

| File | Before | After | Change |
|---|---|---|---|
| `bin/s1.rs` | 2521.5 / 444 lines | 2537.5 / 450 | +16.0 |
| `bin/s2.rs` | 3050.5 / 490 | 3053.0 / 489 | +2.5 |
| `claude/protocol.rs` | 2043.5 / 466 | 2043.5 / 466 | 0 |
| `claude/session.rs` | 3063.0 / 528 | 2981.0 / 511 | -82.0 |
| `tests/replay.rs` | 2027.5 / 372 | 2037.5 / 377 | +10.0 (five literal `turn_started` lines) |
| **Total** | 12706.0 / 2300 | 12652.5 / 2293 | **-53.5 (-0.4%)** |

With fix 1, the total is 12615.5, a drop of 0.7%. `session.rs`'s boundary score rose from 239.0 to 265.0, while no boundary code was added. The `--by fn` output puts none of the 26 points in a function. So the scorer has reclassified item-level code after the `session_id` field and `News` were removed. This is inferred, and I did not chase it. The total still falls.

### Ratchet

No candidate. The diff is small, the one repeated shape is expressible today (fix 1), and there is no friction log.

## 1. The API change

**`TurnStarted` per `system/init` is correct for every caller.** Measured: the committed recordings print one `init` per user message, with the same `session_id` every time (the five added `turn_started` lines in `replay.rs` are literal values from those recordings, and the test passes). The callers in the workspace are `bin/s1.rs` and `tests/replay.rs`. No other crate depends on `agents` yet. `s1`'s `Turn` is one per turn, so writing `session_id` on each `TurnStarted` stores the same value it stored before.

**Nothing loses the session id.** Before, the actor kept the id only to suppress repeats, and nobody could read it. Now every `TurnStarted` and every `TurnFinished` carries it, and a caller keeps whichever it saw last. Resume reads it from the caller: `s1` takes `t.session_id` from turn 1 and passes it as `ClaudeOptions.resume`, and step 4 passed in all five runs with the same id before and after the restart. One case got better. If a future CLI changed the id mid-process (`--fork-session`, for example), the old actor would have emitted a second `SessionStarted`, and a consumer that treated it as once-only would have been wrong. Now the id simply rides on every turn.

**The name fits.** `system/init` is Claude's `turn/started`, and the PROTOCOL note says so. The variant carries the session id rather than a turn id, because Claude has no turn ids.

## 2. Determinism

**What step 3 still proves.** The interrupt is acknowledged, the turn ends `Failed` with an `aborted*` `terminal_reason`, and the session takes the next turn (`pong`). That is the part of the interrupt contract that P1's `TurnEnded(Interrupted)` mapping and the "next `Send` is accepted" rule depend on. The race is now between a local pipe round trip and the model's first token. The first takes milliseconds, and the second needs an API round trip. All ten runs passed step 3. `s2` shows no output before the interrupt in all five runs (`""` and `items=[]`). For `s1` the one raw recording shows the same, and fix 2 makes every later run show it. In practice that is deterministic, and it no longer depends on what the model says.

**What a CLI change could now slip past.**

- **Interrupt mid-stream.** This covers the partial assistant message with `"aborted": true`, the order of the cut-off message, `[Request interrupted by user]` and `result`, and whether any text follows `result`. P1 maps assistant text to events and relies on `AssistantMessage` replacing the deltas. A change here breaks the daemon and passes the drift check. The old step 3 covered it on runs where it got output in time.
- **Interrupt while a tool runs.** The old prompt asked for `sleep 30`/`sleep 60` but interrupted on the first reply, so it never reliably covered this case either. Nothing covers it now.
- **Interrupt before `init`.** The P1 session actor sets `Turn::Running` on `Send`, not on `TurnStarted`, so a user can interrupt in the gap before the CLI reads the message. Nobody has observed what the CLI does then. It may run the turn anyway.

**A deterministic mid-stream check exists once unit 2 lands.** Unit 2 adds `--include-partial-messages` (`StreamDelta`) and Codex `AgentDelta`. With "count from 1 to 200", the first delta arrives seconds before the turn could end. So "interrupt on the first delta" is a step that depends on the model producing any text at all, and not on what it says. The old flake came from waiting for a whole `assistant` message or item, which arrives at the end of a short turn. Add it as step 3c in both binaries. An "interrupt right after `send_user`, before `init`" step is also deterministic and costs one turn. It answers the third gap.

**Step 3b.** It does depend on haiku calling Write. So do steps 1, 2 and 5, so 3b is not a special case. All of them passed in every run I have (ten logs, plus the 2026-11-10 runs recorded in `PROTOCOL.md`). I know of no way to make the CLI send `can_use_tool` without a model tool call. The risk is a red run that is not drift. The detail line already tells the two apart (`permissions=[]` means the model didn't call the tool). If it ever flakes, retry the turn once when `permissions` is empty, rather than adding a three-valued verdict. I would not change it now.

## Impact on the P1 design

- **§2.7, Claude table.** Rename the first row to `TurnStarted { .. }` → none, because the actor sets `Running` on `Send` and `SessionReady` comes from `spawn`. The row now fires once per turn instead of once per process, so state that "none" is deliberate. The Codex `TurnStarted { turn }` row maps to `Step::TurnStarted(turn)`. Two same-named events taking different paths will confuse the next reader unless the row gives the reason.
- **§4.1 and §5 unit 2.** This is the real change. `claude-deny-interrupt.jsonl` is planned as "a long turn that ends `aborted_streaming`", built from unit 2's new `s1` recording of step 3. After this change, that turn has no output, so the recording would show an interrupt with nothing to cut off. E2e `a_denied_request_continues_and_an_interrupt_ends_the_turn` would then never test deltas followed by `TurnCompleted(Interrupted)`. Unit 2's done-when should add the delta-interrupt step (3c above) to `s1` and `s2`, and the recording should come from 3c. `codex-interrupt.jsonl` ("a turn interrupted mid-stream", unit 5) is safe only while it comes from the existing `server1.jsonl`. If `s2` is re-recorded, it needs 3c too.
- **§2.5, interrupt on `Running`.** Record that interrupting a Claude turn before `init` has not been observed. Either add the pre-`init` step, or list it in open questions.
- `p1-u2-agents` already contains these three commits, so unit 2 is building on the rename now.
