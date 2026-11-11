# Review: rewrite of `crates/agents` (rw-agents)

The reviewer's pass over `215a225` (`git diff 078f3bb...rv-agents`, where `078f3bb` is the merge base with `main`) and its friction log, `.audit/rw-agents.friction.md`, under subset version 3. The scratch files for every measurement below are in `docs/subset/measurements/0025/`, `0026/`, `0008-reach/` and `agents-review/`. Rerun any of them with `./target/debug/subset measure <file>`. Every "after" that is code from the crate was applied to a scratch worktree and compiled, linted and tested there, unless the row says otherwise.

## Compliance

`subset check` reports nothing under `crates/agents/`. The workspace still has 700 violations, all in crates not yet rewritten on this branch. `cargo clippy -p agents --all-targets -- -D warnings`, `cargo fmt --check -p agents` and `cargo test -p agents` (31 tests) pass. The crate measures 25,686 tokens and 4,269 lines.

The code holds the qualities in most places: named enums replace every `bool` the old code carried (`VersionPolicy`, `AfterDeny`, `Outcome`, `Presence`, `Verdict`, `Interrupt`), the actors follow 0007's shape, and mutation stays in loop state and library `&mut` APIs. The review-only rules hold for 0006 (every `async fn` awaits I/O or a channel), 0012 (serde derives only on frames in the two `protocol.rs` files), 0013 (`wire` and `encoded` are codec fns with one `Serialize` bound) and 0016 (`#![cfg(unix)]` on whole test crates, which 0016 exempts). The breaches are below.

### Required fixes

Introduced by the rewrite:

1. **One way: equality written as `cmp` on the inner field.** `claude/session.rs:428`, `bin/s2.rs:198`, `bin/s2.rs:555`, `tests/codex_replay.rs:205` and `bin/s1.rs:326` compare two IDs (or a `Vec`) with `.0.cmp(&x.0)` and match `Ordering::Equal`. §A4 already gives the idiom for equality without `Ord`, `.eq(..)` with `.then_some(..)`, and it doesn't reach into `.0`. Measured on all five sites, §A4's form is 14% shorter (355 to 305 tokens, `agents-review/eq-before.rs` and `eq-a4.rs`). The rewrite compiles, passes clippy and `subset check`, and drops the `std::cmp::Ordering` import from three files.
2. **One way: three counting idioms.** `bin/s1.rs:540` and `bin/s2.rs:593` count with a `fold`, and `tests/replay.rs:216-222` maps each line to `1` or `0` and sums. If the owner accepts 0025, all three become `filter_map(..).count()`. If not, `replay.rs` uses the `fold` (§9).
3. **0002: `pub` on what no other crate uses.** No crate in the workspace depends on `agents`, so its only other crates are its own binaries and integration tests. Checked by searching `src/bin/` and `tests/` for each `pub` name:
   - `claude/protocol.rs:32` `PermissionMode::to_cli` and `claude/session.rs:35` `SettingSource::to_cli`. Make them `pub(crate)`. Each returns a `claude_codes` type, so today they also break the module's "nothing from `claude-codes` leaves this module".
   - `codex/server.rs:208` `CodexThread::steer`. No crate calls it, this one included. `pub(crate)` would leave it dead, so the fix is to delete `steer` and `Call::TurnSteer` with its test, unless the owner keeps it for the daemon. This was already `pub` on `main`.

Carried over from `main`, still breaches in the rewritten code:

4. **0001: tuples kept in a field.** `claude/session.rs:309` `pending: HashMap<String, (ControlCall, oneshot::Sender<…>)>` and `codex/server.rs:322` `HashMap<i64, (String, oneshot::Sender<…>)>`. A value that outlives its expression is a named struct. Make each a `Pending { call, reply }` (claude) or `Pending { method, reply }` (codex). The `(_, (_, reply))` patterns at `session.rs:502` and `server.rs:527` become `(_, Pending { reply, .. })`.
5. **Errors are values: an encoding failure reported as a wrong decision.** `codex/protocol.rs:740-742` maps a `serde_json` error to `NotApplicable {}`. `codex/server.rs:500-503` then reports `Error::DecisionNotApplicable` and puts the approval back as if the caller chose a bad decision. Give `answer` an error enum with `NotApplicable` and `Encode(serde_json::Error)`, and pass the second on as `Error::Json`.
6. **Named states: sentinel values.**
   - `codex/protocol.rs:289`, `:313`, `:330` and `claude/protocol.rs:235` turn a failed `to_value` into `Value::Null`, which a reader can't tell from a real `null`. Return the error, and let the caller fall back to `Unknown(raw)` as `parse_line` already does for frames that don't parse.
   - `claude/session.rs:276` and `codex/server.rs:288` report `ExitStatus::default()`, which is exit code 0 (success), when `wait` fails after a kill. The caller sees a clean exit that didn't happen. `Process::Exited` and `ClaudeEvent::Exited`/`CodexEvent::Exited` need to carry the failure, for example as `std::io::Result<ExitStatus>`, or as an enum with a variant for an unknown exit.
   - `claude/protocol.rs:250` uses `"<missing>"` as the version when `initialize` names none, and the error then reads "claude <missing> is not supported". Give `UnsupportedVersion` a variant for an unreported version.

### Advisory, not required

- `codex/protocol.rs:765` says the frame structs' fields are in alphabetical order "the order `serde_json::json!` wrote them in". Nothing depends on that order. JSON objects are unordered, and the unit tests compare parsed `Value`s, not strings. Delete the comment, so no one maintains a constraint that doesn't exist (friction X7).
- `codex/protocol.rs:3` says nothing from `codex-codes` leaves the module, but `server.rs` imports `RequestId` and `methods`. Correct the comment or wrap `RequestId`.
- `bin/s2.rs:154` keeps a `running` counter next to `turns`, whose `outcome`s already say which turns are done. The two can disagree, and `saturating_sub` at `:224` hides a second `TurnCompleted` for one turn. Deriving `Done` from the turns removes the second source of truth.
- `bin/s1.rs:95` and `bin/s2.rs:156`/`:285` store `Option<Result<(), Error>>` in a field. 0005 allows `Result<(), E>` as a return type. Whether 0023's "never a field" covers our own `Result<(), E>` stored for later is for the owner. No change until then.
- `s1.rs` and `s2.rs` each define `Presence`, `Verdict`, `report`, `log_path` and `fresh_scratch`. They are separate binaries, so this is duplication rather than two idioms.

## Clippy against the subset

| Lint | Who wins | Why | Sites |
|---|---|---|---|
| `clippy::single_match` | subset | Its fix is `if let`, which §A1 excludes. The implementer added a `trace!` and a `debug!` arm only to give two `match`es a second arm. | `codex/server.rs:418`, `:449-451` (Y3) |
| `clippy::unnecessary_filter_map` | subset | Its fix is `filter` with a `bool`. §8 prescribes `filter_map` with a `match`, exactly the form the lint rejects. It forced two other counting idioms. | `bin/s1.rs:540`, `bin/s2.rs:593`, `tests/replay.rs:216-222` (S2, Z5) |
| `dropping_copy_types` (rustc) | subset | It forces `.ok();` beside 17 `drop(..)`s, two idioms for discarding a value. The bug it guards against, `drop` of a lock guard's reference, can't occur without interior mutability. | `claude/session.rs:477`, `codex/server.rs:481` (C16, Y6) |
| `clippy::obfuscated_if_else` | clippy | The form it rejects, `b.then_some(x).unwrap_or(y)`, hides a two-way branch in a chain. The subset's form is a `match`, so the two agree. The cost in C1 is the `bool` boundary, below. | `claude/protocol.rs:172`, `:230` (C1) |

This is **proposal 0025**: allow the first three in `[workspace.lints]`, and remove the padding. It cuts 1,139 tokens to 1,098 (-3.6%) over the whole affected functions, and removes five friction points of one kind. The proposal lists every site, the config, and the clippy output with and without it.

## Ratchet

| Candidate | Rows | Before | After | Change | Qualities | Outcome |
|---|---|---|---|---|---|---|
| Allow three lints whose fix the subset excludes | C16, Y3, Y6, S2, Z5 | 1139 / 186 | 1098 / 185 | -3.6%, 5 sites of one kind | all kept, three helped | **Proposal 0025** |
| One `ask<T>` helper per actor handle | C4, Y1 (+ §A11's host.rs) | 932 / 94 | 777 / 92 | -16.6% | all kept, one way helped | **Proposal 0026** |
| `.eq(..).then_some(..)` for equality (§A4, already in the subset) | C11, S4, Z3, Z5, T1 | 355 / 40 | 305 / 39 | -14% | all kept | Required fix 1, not a construct |
| `true`/`false` patterns on a library or std `bool` | C1, C11, S4, Z3, Z5, T1 | 419 / 48 | 344 / 43 | -18% against today's code, -7% against required fix 1 | named states weakened | Not proposed. The core excludes these patterns as "a boolean check written as a match". |
| `json!` in the protocol modules | X5, X6, X7 | 577 / 103 | 380 / 38 | -34% | named states and one way weakened | Not proposed. §A13 decided it. The frame shape stops being a type, and the checker can't read the macro body (the after hides `-32601`, a negative literal). |
| `true`/`false` as a match arm whose match is a library field value | C2 | 86 / 19 | 70 / 12 | -19% | all kept | Not proposed. One site in the workspace. 0023's text puts the literal directly in the field, which `subset check` enforces. |
| Negative literals | X8 | 2 tokens | 11 tokens | | kept | Not proposed. One site in the workspace, and §A5 sends operators to methods. |
| `on_input` without `Input::Closed` (in-subset) | C9, Y2 | 323 / 48 | 313 / 43 | -3% | all kept | Not proposed. No new construct, and the saving is small. The cost of C9 and Y2 is 0007's shape. |
| Widen 0008 (see next section) | Z1, S1, T1 | | | | forward data flow weakened | Not proposed. Evidence for the owner's decision on 0008. |

### Evidence on 0008's reach

The friction log puts the drive loops in tests and binaries at +29% to +96% (R1, S1, T1, Z1). Those numbers compare the subset code against `main`, which used `while`, `for`, `let mut` counters and closures that 0007, §A6 and §A8 rule out whatever 0008 says. To isolate 0008, three widenings were rewritten and measured against the current subset code.

| Widening | Site | Before | After | Change | Checker sites |
|---|---|---|---|---|---|
| (a) `&mut` to a record that loop state owns | `s2.rs` `Run::on`, `Run::apply` and `pushed` (Z1, Z2) | 606 | 510 | **-15.8%** | 4 (`&mut Turn` param, 3 assignments) |
| (b) `&mut` of our type outside loop state | `codex_replay.rs` `Recorder` with `&mut self` called from test bodies (T1) | 2612 | 2570 | -1.6% | 10, and the loop leaves 0007's shape |
| (c) none: split the owned `Live` into `&ClaudeSession` and the library's `&mut Receiver` | whole `s1.rs` (S1, S6) | 2955 | 2952 | -0.1% | 0. This is already allowed. |

- **(a) is the only widening that pays.** In `s2`, a turn record lives in `Run.turns`. Today each event removes the record, rebuilds it with struct update and inserts it back. A `&mut Turn` drops the `pushed` helper and four struct updates. The mutation stays inside the loop state's own methods and is exclusive, the same confinement 0008 already accepted. A rule for it could read: "loop state's methods may change a value the loop state owns, through `&mut` taken from its own fields".
- **(b) buys almost nothing.** Moving `Recorder` through each call costs a `let rec =` per call. Measured, that is 1.6%.
- **(c) shows S1's cost isn't 0008's.** `s1` moves `Live` from turn to turn because `&mut Live` is out. Passing the session by `&` and the event receiver by library `&mut` is already in the subset (`subset check` passes), and it is the same size.

So the measured case for widening 0008 is (a), one site at -16%, plus the proto review's owned I/O wrappers (`FrameBuffer`, -23%). Both are "a value owned by state that is already mutable". Neither is "any `&mut` of our types in tests and binaries". The rewrites are `docs/subset/measurements/0008-reach/`. The (a) and (b) versions compile, and (b) passes the Codex replay tests.

### 0024 (ordering derives)

Agents adds no evidence for 0024. Its five comparisons are equality on IDs whose order means nothing (`SessionId`, `TurnId`, `ThreadId`), which 0024 excludes by its own rule. Its order comparisons are on `usize`. Required fix 1 is consistent with 0024's "never reaches into `.0` to compare".

## Tool notes

- `subset check` accepts a closure passed to any method call, our own included. 0026's `self.ask(|reply| …)` passes, while the free-function form `ask(&commands, |reply| …)` is flagged. `expr.closure.adapter` is meant for iterator, `Option` and `Result` methods only, so the checker is more lenient than the rule here.
- R8 is the proto review's `expr.array` finding at a call site. `recording(name, &[A, B])` is rejected because the array's slot is the `&` operand, not the call argument. The proto review's fix (pass the slot through `&`) covers it.

## Friction rows

The log has 60 rows (C1 to C18, R1 to R8, S1 to S7, X1 to X10, Y1 to Y6, T1 to T5, Z1 to Z6), not 62.

| # | Outcome |
|---|---|
| C1 | Not proposed. Clippy's `obfuscated_if_else` stays on (see "Clippy against the subset"). `true`/`false` patterns were measured at -16% on these two sites, but they weaken named states and the core excludes them. |
| C2 | Not proposed. The arm-per-variant form is what 0023 allows. Moving the literal into a nested `match` saves 19% at this one site, but 0023 puts the literal directly in the field. One site is too few to widen it. |
| C3 | Not proposed. Already decided by §A2 and §A3. |
| C4 | **Covered by proposal 0026.** |
| C5 | Not proposed. Already decided by §A2 and 0007. `log_stderr` is a drive fn in the rule's shape. |
| C6 | Not proposed. Already decided by §A1, which gives this exact match. |
| C7 | Not proposed. Already decided by §C2 (builder rebinding). Too small: 13 tokens. |
| C8 | Not proposed. Already decided by §A2 and §A12. |
| C9 | Not proposed. 0007's shape, accepted at +43%. An in-subset rewrite without `Input::Closed` saves only 3%. |
| C10 | Not proposed. Already decided by §A1 (let chains). Too small: 14 tokens. |
| C11 | Not proposed as a construct. Required fix 1 rewrites it to §A4's `.eq(..).then_some(..)`. |
| C12 | Not proposed. Already decided by 0008 (no compound assignment). |
| C13 | Not proposed. +3%. Already decided by §A2. |
| C14 | No outcome needed. Shorter in the subset. |
| C15 | Not proposed. Already decided by §A11. +7%. |
| C16 | **Covered by proposal 0025** (`dropping_copy_types`). |
| C17 | Not proposed. Already decided by §A9. |
| C18 | Not proposed. Already decided by §A9. |
| R1 | Not proposed. The cost is 0007's loop shape and §A8 (no stateful callback). `Replay` is already loop state, so a wider 0008 doesn't shrink it (inferred, not measured). |
| R2 | Not proposed. Already decided by §A13. |
| R3 | Not proposed. Already decided by §A7 and §A8. |
| R4 | No outcome needed. Shorter in the subset. |
| R5 | No outcome needed. Shorter in the subset. |
| R6 | No outcome needed. Shorter in the subset. |
| R7 | Not proposed. Already decided by §C3. Too small: 7 tokens. |
| R8 | Not proposed. Covered by the proto review's `expr.array` tool fix (see "Tool notes"). |
| S1 | Not proposed. Evidence for 0008's reach: splitting `Live` into borrowed parts is already in the subset and the same size (-0.1%). |
| S2 | **Counting part covered by proposal 0025** (`unnecessary_filter_map`). The `Verdict` enum is §C1 and §C2 working as intended. |
| S3 | No outcome needed. §11 working as intended. |
| S4 | Not proposed as a construct. Required fix 1. |
| S5 | No outcome needed. +0 tokens, §11's preferred form. |
| S6 | Not proposed. A cost of S1's design, not of a missing construct. The split-`Live` rewrite removes it. |
| S7 | Not proposed. Already decided by §A8 and §A13. |
| X1 | No outcome needed. Shorter in the subset. |
| X2 | Not proposed. Already decided by §A2, §A8 and 0015. |
| X3 | Not proposed. +3%. Already decided by §A8. |
| X4 | Not proposed. +7%. Already decided by §A2. |
| X5 | Not proposed. `json!` measured at -34% with X6 and X7, but §A13 decided it, and it weakens named states and one way. |
| X6 | Not proposed. Same as X5. Its `encoded` error mapping is required fix 5. |
| X7 | Not proposed. Same as X5. The alphabetical order is not a real constraint (advisory note). |
| X8 | Not proposed. One site in the workspace. |
| X9 | No outcome needed. Shorter in the subset. |
| X10 | Not proposed. Already decided by §A9. The `ChangeKind` enum is intended. |
| Y1 | **Covered by proposal 0026.** |
| Y2 | Not proposed. Same as C9. |
| Y3 | **Covered by proposal 0025** (`single_match`). |
| Y4 | Not proposed. +3%. Already decided by §A2. |
| Y5 | No outcome needed. Shorter in the subset. |
| Y6 | **Covered by proposal 0025** (`dropping_copy_types`). |
| T1 | Not proposed. Evidence for 0008's reach: `&mut self` on `Recorder` saves 1.6%. Its `.0.cmp` at `codex_replay.rs:205` is required fix 1. |
| T2 | Not proposed. Already decided by §A5 and §A12. |
| T3 | Not proposed. Already decided by §C2 and 0008. |
| T4 | Not proposed. §11 std-bool friction. One site, and no construct keeps named states. |
| T5 | Not proposed. Already decided by §A13 and §C3. |
| Z1 | Not proposed. Weakens forward data flow. Measured -15.8% with `&mut` to a record that loop state owns, and raised for the owner as 0008's reach (a). |
| Z2 | Not proposed. Already decided by 0013. The `pushed` helper disappears under 0008 widening (a). |
| Z3 | Not proposed as a construct. Required fix 1. |
| Z4 | Not proposed. Already decided by §A1 and §C2. Counted in Z1. |
| Z5 | **Counting part covered by proposal 0025.** The resume comparison is required fix 1. |
| Z6 | Not proposed. Already decided by §A13. |
