# Review: session model round C (`session-model-c` at `4bccdc5`)

Pair loop step 2. The round keeps the agent's checklist (Claude
`TodoWrite`, Codex `turn/plan/updated`) as host-owned tasks, and lets the
effort change between turns. The diff is `git diff 0d794c1..session-model-c`
(4 commits). The friction log is `.audit/session-model-c.friction.md`. I
ran no live check (`s1`, `s2`). One offline probe ran the pinned Claude
2.1.316 with `env -i`, a scratch `HOME` and `CLAUDE_CONFIG_DIR`, and
`ANTHROPIC_BASE_URL` pointing at a closed local port, so no request left
the machine. The scratch directory is deleted.

## Verdict

**Changes requested.** The shape is right. Tasks are one pure function
(`tasks::synced`) and one `Step`. The effort is one field on `Choice` plus
the effort the running agent has, and the actor's one-order-at-a-time loop
rules out the races I looked for. `tools/check.sh` exits 0 (fmt, scrub,
clippy native and wasm, all tests). `tools/subset check` exits 0 on its
own too. Four fixes are required: one correctness, two volume and one
docs.

## Required fixes

1. **Undo leaves the undone turn's tasks.** `SessionActor::tasks` and
   `SessionView::tasks` change only on `TasksChanged`
   (`crates/daemon/src/session.rs:1139`, `crates/client/src/host_view.rs`
   `TasksChanged` arm). Failing input: a turn whose `TodoWrite` calls
   write and tick off a checklist, then `UndoTurn` on that turn. The
   transcript, the files and the agent's conversation go back to before
   the turn. The task list still shows the checklist the undone turn
   wrote, with its "1 of 3 done", and it stays that way until the agent
   happens to write a checklist again. A restart restores the same stale
   list, because `state` takes the last `TasksChanged` in the log. Fix: when
   `execute_undo` logs `UndoHistoryRestored` (`session.rs:1509`), take
   the last `TasksChanged` before the checkpoint's `turn_start` from
   `self.log.history()` (empty if there is none). If it differs from
   `self.tasks`, set `self.tasks` to it and log `TasksChanged`. Add the
   case to the undo e2e.

2. **`offers` duplicates `choose`'s effort check and disagrees with it**
   (`session.rs:101-125` against `session.rs:79-95`). Both do the same
   steps: find the model's row, `find` the effort in `efforts`, and build
   the same "`{name}` doesn't take the effort `{e}`" message. They differ
   only where the host lists no efforts for the model. `choose` passes
   the effort through, as its doc says ("unless the host knows nothing of
   the model": a `--claude-model` default or a Codex credential's model
   that the CLI didn't list). `offers` rejects it as "this host lists no
   efforts for …". So a session started at `--effort high` on such a model
   can never have its effort changed, not even back to `high`. The app
   hides this, because it shows no picker when the list is empty. Fix: one
   helper, `takes(models, model, effort) -> Result<(), String>`, holding
   the check and the message. `choose`'s `(Some(effort), Some(choice))`
   arm and `set_effort` both call it, with one rule for an unlisted
   model. Passing the effort through, as `choose` does, keeps
   `architecture.md` §3 "Model and effort" true. If rejecting is wanted
   instead, change `choose` and the doc to match. This also removes about
   20 lines.

3. **`Block::Checklist { id }` is never read**
   (`crates/agents/src/claude/protocol.rs:137-140`, built at `:713`).
   Every match ignores it: `mapping.rs:136`, `s1.rs:319`, `replay.rs:88`,
   and the unit tests' `..`. The `TodoWrite` result's `tool_use_id`
   needs nothing from it either, because the mapping drops a result for a
   call no client saw. Fix: make it `Block::Checklist(Vec<PlanStep>)` and
   drop the `id` from the construction and the mapping test
   (`mapping.rs:629`).

4. **The roadmap puts the new items 9 and 10 inside item 2's list**
   (`docs/roadmap.md:94-105`). They sit between item 2 and item 3.
   CommonMark numbers an ordered list from its first item, so the page
   renders them as 3 and 4, and "Sign in from the app" shows as 5. Fix:
   move items 9 and 10 after item 8.

## Checked and fine

- **Task id rule.** I traced `kept` by hand for every shape the brief
  names, and the unit test covers them. In a reorder (`[a,b,c]` to
  `[c,a]`), each step keeps its id by text and `b` is dropped. With
  duplicate text (`[a,a]` to `[a,a,a]`), the first two keep ids 1 and 2
  and the third is new. When the list shrinks, a task no step keeps is
  dropped. A rename in place (`b` to `b2` at the same position) keeps the
  id by position. Ids in one list are always distinct: by-text indices are
  unique (`taken`), and a position fallback is refused when any step took
  that index by text. One ambiguous case is in owner decision 1.
- **No-op updates.** `synced` gives fresh ULIDs only to new steps, so an
  unchanged list compares equal and logs nothing. A status-only change
  logs once.
- **Restore.** Tasks come back from the last `TasksChanged`
  (`session.rs` `state`), and the actor outlives its agent, so `Continue`
  keeps them in memory. The registry folds `ModelChanged` and
  `EffortChanged` in log order (`registry.rs:515`), so a restart starts
  the CLI at the latest effort. `tasks_and_effort_survive_a_host_restart`
  shows `--effort low` on the second launch and the same ids after it.
- **The effort reaches the provider on every path.** A fresh start,
  `Continue`, undo's restart and a host restart all go through `start`,
  which passes `choice.effort` as `--effort` or `model_reasoning_effort`
  and records it as the agent's `effort`. A running agent gets the
  difference from `changed` just before `send_user` (Claude
  `apply_flag_settings`) or on `turn/start` (Codex). The field is
  updated only after the provider accepted. If Claude's
  `apply_flag_settings` fails, the turn fails with that message and the
  next send tries again. A pinned CLI that refused it would fail every
  turn. It is pinned, so that is acceptable.
- **SetEffort races.** Orders run one at a time in the actor. During a
  turn, `SetEffort` changes `choice` only, and the turn keeps its effort
  (the e2e checks that nothing is written to the CLI until the next
  message). During undo recovery it gets `UndoInProgress`, like every
  other order. `ModelChanged` at `Continue` (`offered`) checks the effort
  again against the new default and drops it if that model doesn't take
  it, so the effort always fits the model. `SetEffort` is checked against
  the session's model in the provider's current list. A model the new
  credential no longer offers is rejected until `Continue` swaps it.
- **`apply_flag_settings` side effects** (probe). From `--effort high`,
  `apply_flag_settings {effortLevel: "low"}` answered `success` with no
  `response`. `get_settings` then showed `applied.effort: "low"` from the
  source `flagSettings`. No `settings.json` was written to the scratch
  `CLAUDE_CONFIG_DIR` or `HOME`, so the change stays in the process and
  does not reach other sessions that share the private home.
- **The CLI switches.** `CLAUDE_CODE_ENABLE_TODO_TOOLS=1` and
  `CLAUDE_CODE_ENABLE_TASKS=0` only choose which checklist tool the model
  gets (Claude `PROTOCOL.md`). The tools they turn off, `TaskCreate` and
  the rest, were never on for a 5.x model. The variables are inherited by
  the agent's own subprocesses, which is harmless. Codex's
  `tools.update_plan.enabled` is in the config of both `thread/start` and
  `thread/resume`, so a resumed thread keeps the tool.
- **Subagents.** Claude frames with `parent_tool_use_id` stay `Unknown`.
  `a_subagents_checklist_is_not_the_sessions` replays the probe and gets
  zero checklists. A Codex subagent runs on its own thread id, which the
  hub has no route for (`codex_hub.rs:307`), so its plan is dropped too.
  Codex subagents were not recorded, and `PROTOCOL.md` says so.
- **Compatibility.** The log is postcard, so a variant is its index.
  `TasksChanged`, `EffortChanged` and `SetEffort` are appended at the end
  of their enums, so stored logs still decode. The ALPN bump to
  `conductor/11` covers the wire change.
- **`ThreadEvent::SettingsChanged`** only gets a trace in the daemon, but
  `s2` and the Codex replay use it as the provider's confirmation of the
  effort, as `get_settings` is for Claude. Keep it.
- **Docs.** Architecture §3 (commands, events, "Effort between turns",
  "Tasks", the provider table) and both `PROTOCOL.md` files match the
  code.
- **Subset and friction.** Clean. Each friction entry is written the long
  way with listed constructs, and none works around the subset. Entry 1
  (`let _ = effort.replace(change)`) and entry 9 (a `nth` closure for
  indexing) are the clumsiest results and worth carrying to the subset
  repository.

## Optional

- **The effort picker can show an effort the host refused**
  (`crates/app/src/session.rs:179-210`). The `select` is uncontrolled. When
  `SetEffort` is rejected (`ProviderUnavailable`, `NotOffered`),
  `session.effort` doesn't change, so nothing re-renders and the select
  keeps the refused value. The notice shows the error, but the header
  then disagrees with the session. Re-key the select on the session's
  effort, or reset its value when the command is rejected.
- `Registry::relaunch`'s doc says "for a `Continue`" (`registry.rs:327`).
  `UndoTurn` and `SetEffort` now use it too. Say "for an order that may
  start the agent, or check against what the provider offers".
- The browser run: I did not run
  `tools/e2e-browser.sh`. The committed `phone-light-tasks.png` shows the
  tasks and the effort select at 390 px without overlap.

## Owner decisions

1. **A rename together with a removal hands a task's id to another
   task.** `[a,b,c]` to `[a,c']` (b done and dropped, c reworded) gives
   `c'` the id of `b`, by position. The rule is documented, and without
   fuzzy matching no rule can tell this apart from "b reworded, c
   dropped". It matters once #30 hands tasks between sessions by id. One
   option: keep an id by position only when the list's length didn't
   change. That gives fewer wrong matches and more new ids.
2. **Should `SetEffort` be refused while undo recovery is pending?** It
   is today (`UndoInProgress`). It touches neither files nor history, so
   it could be accepted. §3 doesn't say.
