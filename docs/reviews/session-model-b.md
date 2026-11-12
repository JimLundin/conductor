# Review: session model round B (`rv-session-model-b` at `c667683`)

Pair loop step 2. The round adds reasoning text, a model and an effort per
session, and `Continue`. The diff is `git diff main rv-session-model-b`.
The friction log is `.audit/session-b.friction.md`. Every real CLI run
below used `env -i` with a scratch `HOME` and `CLAUDE_CONFIG_DIR`. Nothing
signed in, and no request left the machine. Probes that needed code ran in
a scratch copy of the branch with its own `target`, which is deleted.
Scores come from `tools/subset score`. Scratch edits for the volume
numbers pass `tools/subset check` but were not compiled.

## Verdict

**Changes requested.** The shape is right. The session actor now outlives
its agent, and that one change gives `Continue` without a new layer.
`SUBSET=1 tools/check.sh` passes (fmt, clippy native and wasm, tests), and
so does `tools/subset check`. Requests open at a crash stay withdrawn.
`Pending::native` returns `None` for a resolved request, so no new
request can match an old one. A reply to an old request gets
`UnknownRequest` after a `Continue`. A gateway's empty Claude thinking
shows nothing (probe 1 below). Seven fixes are required: four
correctness, one accessibility, one test and one volume.

The round adds 11222 to the score: 108042.5 on the branch against 96820.5
on `main`, over all of `crates/`.

## Required fixes

1. **A failed start or `Continue` hides the CLI's reason.** Failing
   input: `--resume` with a transcript that is not in the private home.
   The pinned 2.1.316 does not answer `initialize`. It prints one `result`
   frame with `subtype: "error_during_execution"` and `errors: ["No
   conversation found with session ID: <id>"]`, writes the same text to
   stderr, and exits 1 (measured, scratch home). I replayed that frame
   through the fake CLI in the daemon e2e. The session logged
   `FailedToStart { message: "the claude process has exited" }`
   (measured). The reason is only in the host's log. The same path hides
   a pin that drops the hidden flag. `claude -p --bogus-display-flag x`
   exits 1 with `error: unknown option` (measured), so a CLI without
   `--thinking-display` fails every session and the model probe with the
   same "has exited" message. Fix: when the process exits before it
   answers `initialize`, put the `errors` of a `result` frame it printed
   into the error. If there is no such frame, use the last stderr line.
   Add the recorded frame as a fixture and a test for it.

2. **Reasoning that was streaming when the session closed shows
   "Thinking…" forever.** Failing input in `host_view`: `SessionCreated`,
   `SessionReady`, `ReasoningDelta("Look")`, `SessionClosed`. The fold
   leaves `Reasoning { text: "Look", state: Streaming }` (measured with a
   scratch unit test). The app renders that as the pulsing `data-state=
   "streaming"` disclosure under the closed note. A Codex app-server
   killed during a summary does this to every live client. A streaming
   `Reply` already behaved this way before the round. Fix: make
   `SessionClosed` settle the last item (`Item::settled`), as `push` does.

3. **The Codex model list goes stale when a credential is set while the
   host runs.** `CodexHub::models` runs once in `main`, with the stored
   credential. `Configure` restarts the app-server for a new credential
   but never lists the models again. Failing input: start a host with no
   Codex credential, then `Register` a `CodexCredential::Provider`. The
   `Welcome` keeps the list from before the credential. Codex
   `PROTOCOL.md` records nine models, with `gpt-6.1-sol` as the default,
   through the owner's provider. Only the default follows the credential
   (`RegistryActor::models`). Which list the CLI gives with no credential
   is a guess: an empty list (then the picker offers only "Default") or
   the OpenAI catalog (then the picker offers models the gateway may not
   serve). This hits the owner on first setup and on every change of
   provider, until the host restarts. Fix: the hub lists the models again
   after `Configure`, and the registry keeps that list in place of
   `Launcher::Ready.models`. Claude has the same gap. Its probe uses the
   credential stored at start. That is owner decision 4.

4. **The killed Codex e2e covers a path the recording doesn't show.** The
   live run, `fixtures/codex/server4-chosen-killed.jsonl`, was killed
   mid-turn: `turn/start` had been answered (line 113), `turn/started`
   had arrived (line 115), and agent deltas were streaming. The e2e
   recording, `fixtures/e2e/codex-killed.jsonl`, dies at the next
   `turn/start`. That is why `a_killed_codex_server_continues_the_thread`
   expects `TurnCompleted(Failed "the codex app-server has exited")`
   before the close. The ordering is real and harmless. A kill that lands
   during `turn/start` shows a failed-turn line and then the closed note.
   But the recorded case, `Turn::Running` at the close with deltas in
   flight, has no Codex e2e. Fix: `build-codex.py` kills after the
   recorded deltas, as `build-claude.py` does for Claude. Keep the
   turn/start case only if it is wanted as a second test.

5. **The streaming disclosure's `summary` has `role: "status"`.** A role
   on `summary` replaces its native disclosure role (ARIA in HTML lists no
   permitted roles for `summary`). A screen reader then reads it as a live
   region, not a control that expands. Fix: remove the role. The working
   line already says the agent is busy. If an announcement is wanted, put
   a `span { role: "status" }` inside the `summary`.

6. **Focus is lost on close and on `Continue`.** Tapping Continue removes
   the button, because the status leaves `Closed`. Nothing moves focus,
   so it falls to the page (I read this from the code and did not run
   it). The earlier rounds move focus to the answered card and check it
   in `browser.mjs`. Fix: once `SessionReady` follows a `Continue`,
   focus the composer. Add the same check to the `continue` scenario.
   Double taps are owner decision 1.

7. **Two picker entry types where none is needed** (friction 3). The
   `option` value is a string anyway. The model picker is
   `models.choices` with one "Default" `option` written first, and the
   effort picker is the model's `Vec<Effort>`. Replace `ModelPick`,
   `EffortPick`, `model_picks` and `effort_picks` with `default_label`
   and `efforts`. `kept_effort` becomes `effort.filter(|effort|
   efforts(models, model).contains(effort))`. Measured on a scratch copy:
   `new_session.rs` plus `sessions.rs` go from 3172.0 to 3037.5, a saving
   of 134.5, before the test shrinks too. The scratch copy passes
   `tools/subset check` but was not compiled.

## Checked and fine

- **The hidden flag.** Pass it always. The host can't see whether betas
  are on, because that is the CLI's environment. With betas off the flag
  changes nothing (Claude `PROTOCOL.md`). A conditional would add a branch
  and remove no risk. Two things catch its removal. First, a pin needs a
  fixture recorded at that version (`install.rs`, the pin test), and `s1`
  spawns through `command()`, so an `s1` run on a CLI without the flag
  fails at spawn. Second, `s1` itself. The gap is a host on an unpinned
  CLI (`VersionPolicy::AllowUntested`). Fix 1 makes that failure readable.
- **Gateway thinking.** I replayed `fixtures/e2e/claude-killed.jsonl`,
  which has one `thinking_delta` of `""` and a `thinking` block of `""`.
  It gives zero `Reasoning` and zero `ReasoningDelta` events, and no
  `Item::Reasoning` in the fold (probe 1, a scratch e2e test, passed). No
  committed test checks this end to end. The mapping unit test covers
  `ThinkingDelta("")` and `Block::Thinking("")`. Worth adding the probe
  as a test when fix 1 adds its fixture.
- **The probe fixture.** `probe-thinking.jsonl` is the CLI's own stdout.
  The proxy rewrote only the request, so the bytes the adapter parses are
  real. Frame order: the `assistant` thinking message comes before the
  first `text_delta`, so the fold makes one Reasoning item and then one
  Reply. Its labels are in `PROTOCOL.md` and in the doc comment of
  `summarized_thinking_recording`. The derived `claude-reasoning.jsonl`
  drives the daemon e2e and the browser e2e as ordinary traffic. Nit:
  say in the doc comment of `claude_reasoning_streams_live_and_is_stored_
  once_finished` and in the browser scenario that this traffic needs
  betas on, which the owner's gateway refuses.
- **Two clients press Continue.** Orders reach the session actor one at a
  time. The first starts the agent inline, and the second then sees an
  agent and gets `SessionOpen`. The same happens to a double tap. Owner
  decision 1.
- **Continue during a Codex credential restart.** A session can only be
  `Down` after its route delivered `Stopped`. The hub sends that from
  inside `Configure`, so the `Start` of a `Continue` always reaches the
  hub after `Configure`, and `up()` starts the new server. No race.
- **A session that closes again during resume.** If the CLI dies after
  `initialize`, the log shows `SessionReady` and then `SessionClosed`. If
  it dies before, it shows `FailedToStart` (fix 1 covers the message).
  The conversation id survives a failed `Continue`, so the next one tries
  the same conversation again.
- **Rejections.** `UnknownSession`, `SessionOpen` and
  `ProviderUnavailable` cover every case except a model the provider no
  longer serves (owner decision 2). `Continue` answers `Accepted` even
  when the start then fails. The failure is in the log as
  `FailedToStart`, which matches "`SessionReady` follows in the log".
- **Default precedence.** The code does what the brief asks: the Codex
  credential's model (`RegistryActor::models`), then `--claude-model` or
  `--codex-model`, then the CLI's own default (`ready` in `main.rs`).
  `choose` turns "Default" into a concrete model when the session starts,
  so `SessionCreated` and resume both name it.
- **`NotOffered` with an empty list.** A named model gets `NotOffered`.
  An effort is passed through unchecked, because no model is listed. The
  picker then offers only "Default" for both, so the app never sends
  either. That is consistent.
- **Reasoning size.** No cap, and none is needed. A summary is stored
  like an `AssistantMessage`, which has no cap either. The wire limit is
  `MAX_FRAME_LEN`, 16 MiB per frame. The summaries recorded so far are
  132 and 420 characters.
- **The delta arms** (friction 2). The real duplicate is
  `SessionView::reply` and `SessionView::reasoning` in `host_view.rs`.
  They are the same function, with a score of 101.5 each. The friction
  log does not mention it. Merging them, with an `of: Voice` on
  `Item::Reply` and a single `EventKind::Delta { of, text }`, measured
  5842.0 to 5789.0 in `host_view.rs` and 1222.0 to 1182.5 in `log.rs`.
  That saves 92.5 before the new `Voice` enum and the app's arms are
  counted. Not worth a change. Keep both arms.
- **Phone, 390 px.** The committed idle and running screenshots show the
  model next to Interrupt without overlap. The title truncates first. The
  picker screenshot (`*-picker.png`) is written only to the scratch
  directory, so I did not see the form at 390 px. Commit it with the
  other screens. Nit: the header shows the model id (`opus`,
  `claude-fable-5[1m]`), but the picker shows the name (`Opus`, `Fable`).
  `model_name` exists already and could do both.
- **Subset.** Clean. Friction points 1 and 4 to 11 are written the long
  way, as the subset requires. Nit: `model_name` can be private.

## Owner decisions

1. **Should `Continue` be idempotent?** Today a second `Continue`, from a
   double tap or a second phone, gets "the session is still open". The
   app shows that as an error, but the session is running, which is what
   the user asked for. My recommendation: answer `Accepted` when the agent
   already runs, and drop `SessionOpen`. Then every press ends in the same
   state.
2. **A Codex session whose model the new credential doesn't serve.** A
   session closed by a credential change resumes with the model chosen
   at its start, which may be the old provider's default. The turn then
   fails on the gateway. Options: refuse `Continue` as `NotOffered`,
   resume on the new default, or keep things as they are.
3. **A transcript that is gone.** With fix 1 the user sees why. Should
   `Continue` then offer a fresh conversation in the same session, or
   leave the session closed?
4. **Claude's list after a credential change.** The probe at host start
   uses the stored credential. Should a new Claude credential trigger a
   new probe too? Whether the list depends on the credential is not
   measured.
