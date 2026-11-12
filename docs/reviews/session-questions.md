# Review: questions and approval for the session (`rv-session-questions` at `dc26c04`)

Pair loop step 2, roadmap 2, round A. The diff is `git diff main
rv-session-questions`. The friction log is `.audit/questions.friction.md`.
Every run used the fixtures and the fake CLIs. `s1`, `s2` and
`tools/live-host.sh` were not run, and nothing under `~/.claude` or
`~/.codex` was read. Token counts come from `tools/subset measure` on this
branch, on `main`'s copies, and on scratch copies in `/tmp` with a fix
applied.

## Verdict

**Changes requested.** The request lifecycle is right. `SUBSET=1
tools/check.sh` passes and `tools/subset check` is clean. Five fixes are
required: one leak, one unobserved claim, one accessibility gap and two
small volume fixes. Three calls are the owner's.

What holds, checked against the code and the tests:

- **First reply wins across clients.** `SessionActor::reply` handles
  approvals and questions with one path. The e2e test
  `a_claude_question_is_answered_once_by_the_first_client` has a second
  client (`desk`) refused with `AlreadyResolved { by: <first client> }`.
- **Withdrawal.** Questions are `Pending::Open` like approvals, so
  `Step::TurnEnded` withdraws them after `TurnCompleted`. That is the same
  order and the same code as the earlier withdraw-on-turn-end fix
  (`9e9e648`). Claude's `control_cancel_request` arrives before `result`
  and maps to `Step::Withdraw`. Codex's later `serverRequest/resolved`
  finds nothing open. Both are covered by e2e tests and by the `s1`
  recording replay.
- **`WrongReply`.** `fits` refuses a decision for a question, answers for
  an approval, the wrong count, and `ApproveForSession` with no
  `for_session`. A unit test covers each case, and the e2e test checks the
  first two.
- **Answer formats.** Claude keys by question text and joins with ", ".
  Codex keys by id. Each format has a unit test, and an e2e test checks
  the bytes sent to the stand-in's stdin.
- **Claude's session grant.** `grant` keeps only `addRules` entries with
  `behavior: allow` and rewrites their destination to `session`.
  `addDirectories` and any unknown type fail the `Suggestion` parse and
  are dropped. `setMode` is used only when no rule is suggested. The e2e
  test checks the exact `updatedPermissions` sent. The fixture holds only
  CLI stdout, so it can't show that `.claude/settings.local.json` was
  never written. What it does show: the second `touch t1.txt` ran with no
  request, and `permissionMode` stayed `default` in all four `result`
  lines, so no `setMode` went out. The claim that no file was written
  rests on the probe in Claude `PROTOCOL.md`.
- **The hidden `AskUserQuestion` call.** It is hidden in
  `mapping::claude_block`, not in the UI. Its `ToolFinished` is still
  logged for a call no client saw start, and `SessionView` ignores it.
- **390 px.** The browser run (WebKit, 390x844, `docs/p1/browser-run.txt`)
  measures an option as a thumb-sized target. Inputs are 16px, so iOS
  does not zoom.

## Required fixes

1. **A Codex secret question is shown, logged and replayed in clear
   text.** `WireQuestion` drops `isSecret`. Failing input: `USER_INPUT` in
   `codex/protocol.rs` tests with `"isSecret": true`. The card shows a
   plain text field, and the typed answer goes into
   `Resolution::Answered`. That event is durable, so every client receives
   it again on catch-up. No recording has `isSecret: true` (inferred from
   the wire field only). The least code is to read it as `Option<bool>`,
   as friction 1 does, and decline a request that has a secret question,
   with an `Error` saying why. Masking is more code for a case no run has
   shown.
2. **Codex `FileChange` offers a session grant no run has seen.**
   `approval_step` sets `for_session: Some("changes to these files")`.
   Codex `PROTOCOL.md` says no `item/fileChange/requestApproval` ever
   arrived, and only `acceptForSession` on a command was probed. The card
   also lists no files (the request names none), so "these files" points
   at nothing. Set `None` until `s2` records one. This removes one literal.
3. **Focus is lost after answering.** "Send answer" and the approval
   buttons unmount once `RequestResolved` arrives, and focus falls to
   `body`. Nothing announces "Answered by …". `credentials.rs:132` already
   moves focus with `document::eval` after its button goes. Do the same
   for the resolved card. Also, the legend reads "ColorWhich color do you
   prefer?" with no separator, and `role: "group"` on `.options` repeats
   the `fieldset`.
4. **`Behavior` has two variants that exist only to parse (friction 4).**
   This has a better current form. Write `enum Behavior { Allow }`: a
   `deny` or `ask` entry then fails the `Suggestion` parse, and the
   existing `filter_map` drops it. The `AddRules` arm then no longer
   matches on `behavior`. Measured on a scratch copy of
   `claude/protocol.rs`, this saves 17 tokens and 5 lines.
5. **`ASK_USER_QUESTION` is defined twice**, in `agents::claude::protocol`
   and in `daemon::mapping`. The tool's name is a fact about the CLI, so
   export it from `agents::claude` and use it in the mapping. This saves
   one definition and keeps the two copies from drifting.

## Owner decisions

- **The `setMode` fallback.** For Write in `default` mode, the only
  suggestion is `setMode acceptEdits`. Approving for the session then
  accepts every file edit for the rest of the session, not just this one.
  The card says "Allows `acceptEdits mode` until the session ends", which
  names the mechanism, not the effect. It also changes the session's mode
  without the daemon knowing, and the daemon still holds
  `PermissionMode::Default`. That goes stale once modes become a typed
  option. Recommendation: drop the fallback and offer no session grant
  when no rule is suggested. Measured on a scratch copy, on top of fix 4,
  this saves 226 tokens and 41 lines (the `SetMode` variants, the mode
  search, its arm and its test). If it stays, the label should say "all
  file edits in this session".
- **Codex's unlisted `acceptForSession`.** It is not guarded in code. The
  guard is `s2` step 8, which §6.2 runs at every pin bump, and it fails if
  the same command asks again. Offering it only when `availableDecisions`
  lists it would remove it for every command in 0.166, which never lists
  it. Recommendation: keep it, with fix 2 limiting it to commands and
  permissions. Permissions' `scope: session` was in the code before this
  branch.
- **Two Claude questions with the same text.** `answered` collects into a
  map keyed by text, so the second answer silently replaces the first.
  The CLI's own format is keyed by text, so no correct encoding exists.
  The CLI's input schema may refuse duplicate texts before `can_use_tool`
  (unverified: no pinned binary was available to inspect). Ask the
  coordinator to try it by hand in `s1`. If the CLI accepts duplicates,
  refuse such a request in `control_request`.

## Checked and left as is

- **`Resolution` shared by questions and approvals.** The impossible cases
  cost two arms: the app's `Decided` arm for a question and its `Answered`
  arm for an approval, about 40 tokens. A typed split needs a second
  resolution enum, a second `RequestResolved` event, and a branch in
  `withdrawn` on what was asked. Each of those alone costs more than the
  two arms. Keep the shared shape.
- **The Codex `warning`.** It is swallowed at `trace` as an
  unmapped frame in `mapping.rs`. That is fine, because the user can't act on it.
  If a later pin renames or drops the flag, Codex either refuses
  `thread/start` or silently stops asking questions. `s2` step 7 catches
  both. `check.sh` catches neither, because `codex_replay.rs` does not
  replay `server3-questions.jsonl`. The fixture reaches the tests only
  through `build-codex.py`. Replaying it with an assertion on the warning
  text would be a pin↔fixture check. That is optional, since the pin-bump
  procedure already runs `s2`.
- **Loose answers.** `fits` accepts an empty `Answer` and several labels
  for a `Selection::One` question. `Draft` never sends either one, so only
  a hand-written client can. The message "0 answers for 1 questions" has a
  plural slip.
- **Volume.** The branch grows the changed `.rs` files from 56,901 to
  68,197 tokens. Most of that is tests and fixtures-driven e2e cases, and
  each one asserts an observed behaviour. Apart from fixes 4 and 5 and the
  fallback, I found no speculative layer. The `Change::Ask(call,
  Approval)` and `RequestState::changed` refactor in `host_view.rs`
  removes the per-field rebuilds.

## Friction points

| # | Class | Note |
|---|---|---|
| 1 | Real gap | `serde(default)` and a `bool` field. |
| 2 | Real gap, possibly avoidable | Use one `Allow` variant if the CLI accepts `"updatedPermissions": []` on every allow. That needs an `s1` probe. |
| 3 | Real gap | `serde(other)`. The double parse works. |
| 4 | Better current form | A one-variant `Behavior` (fix 4). |
| 5 | Better current form, in part | The nested match is right, but `match name.as_str() { ASK_USER_QUESTION => None, _ => … tool_kind(name, detail) }` moves `name` with no `String::from` rebuild. Checked with `rustc` 2024. It needs `_` to be allowed on a `&str` match. |
| 6 | Real gap | Local closures. |
| 7, 8 | Real gap | `&mut self` outside actors. |
| 9 | Real gap | `bool` props. |
| 10 | Real gap | Array literal outside a call. |
| 11 | Real gap, no cost | Closures in tests. |
| 12 | Real gap, no cost | `count()` matched with `1` is stricter than `Some(true)` anyway. |
