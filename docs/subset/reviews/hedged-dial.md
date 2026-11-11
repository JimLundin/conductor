# Review: the hedged dial (hedged-dial)

The reviewer's pass over `1c45822` and `cd625cb` (`git diff c0bb6fc rv-hedged-dial`, where `c0bb6fc` is the merge base with `main`) and the friction log `.audit/hedged-dial.friction.md`, under subset version 5. The scratch files for every measurement are in `docs/subset/measurements/hedged-dial-review/`. Each `.rs` file there is the hedge section of `link.rs` (lines 754 to 843 at the tip) in one form, and `subset score <file> --by file` reprints its row. `destructured.patch` applies to the tip.

**Verdict: changes requested.** The hedge does what `50827cd` asks for, and the test proves it. Four fixes are required, and there is no proposal.

## What I ran

- `subset check`: no findings in `crates/client/src/link.rs` and none in the new lines of `crates/daemon/tests/e2e.rs`. The rest of the workspace still has its rewrite findings.
- `cargo test -p daemon --test e2e`: all 6 tests pass. The new test finishes in about 1.65 s, against its 3 s budget.
- In a scratch worktree, I set `HEDGE_AFTER` to 60 s. The new test then fails at `e2e.rs:344` with `live within the hedge delay plus a connect: Elapsed(())`. That is the right failure.
- In a scratch worktree with the destructured form (below), I ran fmt, clippy `-D warnings` for `client`, `subset check` (clean) and the new test 8 times. All 8 runs passed, in 1.62 s to 1.77 s.
- I instrumented `retire` and the test with tracing to watch the endpoints close. The results are under "Correctness".

## Correctness

**The race.** Each path ends with one endpoint kept and the other closed. If the first dial returns before 1.5 s, nothing is bound. If the bind fails, the hedge falls back to waiting on the first dial. If one dial fails, its endpoint is retired and the other dial decides. If both fail, the link keeps the endpoint of the dial that failed last. The losing future is dropped in `settle`, so no task leaks. `retire` spawns a detached `close`. I measured one retired endpoint (the stalled one) and it took 3.0 s to close, which is iroh's draining probe timeout. It then reported `is_closed() == true`.

**Growth.** Each dial that runs past 1.5 s binds one endpoint and retires one. The number of live endpoints stays at one, plus at most one or two that are still closing within those 3 s. Repeated reconnects therefore do not pile up endpoints. They do churn them: while the host is down and each dial takes longer than 1.5 s to fail, every backoff cycle binds a fresh endpoint with a new identity, a relay connection, and (native) mDNS and pkarr publishing. That is acceptable for now, but it is noted here in case a host that is down for a long time turns up in the logs.

**The browser and the host.** The new identity has no effect on the host beyond its log lines. `Conductor::accept` (`crates/daemon/src/server.rs:60`) uses `remote_id()` only for `tracing`. Authentication is the token in `Hello`. The WebRTC upgrade takes the browser's fingerprint over the authenticated iroh stream (`crates/daemon/src/rtc.rs:197`), and the client's `upgrade.rs` uses only the `Connection`. No session is keyed by endpoint id.

**Ownership on stop: a regression (required fix 1).** `HostLink::spawn` now documents that the link owns `endpoint`, but the link never closes the endpoint it runs on. Before this change, a caller that kept a clone could close the endpoint at exit, as `crates/client/examples/headless.rs:85-86` does (`drop(link); endpoint.close().await`). After a hedge, the link runs on an endpoint that only it holds. The caller's clone is the retired one, and closing it again is a no-op. I measured this in the scratch test by dropping the `HostLink` one second after `Live`. iroh then logged `ERROR iroh::socket: Endpoint dropped without calling Endpoint::close. Aborting ungracefully.`. By iroh's documentation of `Endpoint::close`, the host's side of that connection then times out instead of closing (inferred, not measured). The stalled endpoint, kept alive by a test clone, was not the one dropped. Without a hedge the old behaviour holds, so this shows up only on the exact path the change adds.

## The test

The test is deterministic and honest. The stalled endpoint has `clear_ip_transports()` and one relay, a `TcpListener` that accepts the connection and never answers. The only way it can reach `Live` is through a second endpoint. Without the hedge, it fails on the timeout and not on an assertion about states (measured above). Its margin is about 1.35 s.

It is not hermetic, though. The hedge binds through `platform::bind_endpoint()` (`presets::N0`, the n0 relays, and mDNS on native), so this test contacts n0's relays and DNS. The file's header says "with relays off so the test needs no network", and that is no longer true of this test. The root cause is in the design: the link binds its second endpoint from a global, so it cannot reproduce how the caller configured the first one. See recommendation A.

## Friction 2: `mut first: Attempt`

**Ruling: a checker gap. The destructured form is a required fix (required fix 2), and no proposal follows.**

The text is clear. 0008(b) lets `let mut` or a `mut` pattern bind "a value of a type from outside our crates", and names pinned futures among its examples. `first` is an `Attempt`, which is ours. `let Attempt { endpoint, mut connecting } = ..` binds the `BoxFuture` itself, so it is inside the text as written. The checker accepts the `mut Attempt` form because, as `docs/subset/compromises.md` ("Whose type it is") records, it approximates 0008(b) as "never reassigned, only lent as `&mut`" and doesn't look at the binding's type. A typed parameter (`mut first: Attempt`) and a binding from `Attempt::start(..)` both name the type in the syntax, so the next tool pass can catch both cases.

Then I measured whether a proposal would allow it. The hedge section is scored with `subset score`:

| Form | Score | Tokens | Lines |
|---|---|---|---|
| destructured (`hedge-destructured.rs`), inside v5 | 483.5 | 515 | 85 |
| `mut` binding of `Attempt` (`hedge-mut-binding.rs`, the tip) | 449.0 | 466 | 72 |
| change | -34.5 (-7.1%) | -49 (-9.5%) | -13 |

The score falls 7.1%, below the 15% bar, and the log has one friction point of this kind, not three. The `mut` form is not proposed. The destructured form costs 13 lines and 49 tokens more than the tip. That matches the implementer's estimate of about 12 lines. `destructured.patch` is the exact change. It compiles, it is clean under clippy and `subset check`, and the test passes with it.

## The `docs/s3/` run records

**Revert them (required fix 4).** They are regenerated output with new timestamps, endpoint ids and temp paths, plus a new iPhone screenshot. No line in either record shows that a hedge fired, so they are no evidence of this change. Their single-sample numbers also read like a regression that nobody attributed to anything: Chromium's first connect went from 1142 ms to 4714 ms and the reload from 929 ms to 1836 ms. Earlier units reverted these files for the same reason. If the owner wants a record that the hedge fires in the browser, that belongs with the stall-injection runs on `reconnect-forensics`, not in `docs/s3/`.

## Compliance and volume

`link.rs` grows from 4896 to 5392 tokens (+496) and its score from 4620.0 to 5100.5. The hedge section is 466 of those tokens. With required fix 2, the file is 5441 tokens.

1. Named states: kept. `Attempt` and `Dialed` name the two shapes.
2. Visible branching: kept. Every decision is a `select!` or a `match`.
3. Forward data flow: kept. The endpoint flows out through `Dialed`, and the loop state takes it.
4. Errors are values: kept. The losing dial's error goes to `debug` and the last error is returned.
5. Local reasoning: weakened, by required fix 1 and recommendation A. `spawn` closes an endpoint the caller may still hold, and the link binds endpoints that the caller didn't configure.
6. One way: breached once in the test (required fix 3).
7. Repetition is data: kept. `race` has two mirror arms, below the 4-case bar.

**Is a smaller form possible?** I measured one. `futures_util::future::select` hands back the losing future, so it needs neither `mut` nor `&mut` (`hedge-future-select.rs`). It scores 501.0 for 521 tokens, worse than both forms above, because it trades the boundary-weighted `select!` body for `Either` patterns and struct updates. It would also add a direct dependency. No other cut keeps the behaviour. `dial` exists because the connect future must own its endpoint, and an `async` block is allowed only as a spawn argument. `finish` has two callers. The volume is lean for what it does.

## Required fixes

1. **Close the endpoint the link runs on.** Give `HostLink` a way to stop that closes its current endpoint and that a caller can await, for example a `close` request through `ask`, which replies after `self.endpoint.close().await`. Then `headless.rs` calls it instead of `drop(link); endpoint.close().await`, and `spawn` takes the endpoint without the caller keeping a clone. A `Drop` that spawns `retire` would not be enough, because the headless runtime can exit before that task runs.
2. **Destructure `Attempt` in `dial_hedged` and `race`** as in `docs/subset/measurements/hedged-dial-review/destructured.patch` (0008(b), friction 2).
3. **`e2e.rs:331`.** Write `.parse::<RelayUrl>()` and drop `use std::str::FromStr`. The codebase already writes it that way (`crates/proto/src/relay.rs:28`, `:34`), and A8 makes the turbofish the form for a typed binding. This also settles friction 3.
4. **Revert `cd625cb`** (`docs/s3/browser-run.txt`, `native-run.txt`, `iphone-390x844.png`).

## Recommendations, not required

- **A. The link binds what the caller configured.** If the link may replace its endpoint, it should be able to bind the same kind of endpoint. One option is for `LinkOptions` to carry the endpoint's configuration as data (relay mode, address lookup) and for the link to bind both endpoints from it, which also settles required fix 1's ownership question. Until then, update the `e2e.rs` header or say in the new test that its hedge endpoint uses the n0 defaults.
- **B. Tool pass.** Teach `subset check` to reject `mut` on a parameter whose type is ours, and on a `let` whose initializer is a constructor or associated fn of a type of ours, outside loop state and the `endpoints` list.

## Friction log

| Point | Outcome |
|---|---|
| 1. Mirror `select!` arms in `race` | Not proposed. Two arms, below the 4-case bar. The `future::select` form that would merge the plumbing scores worse (501.0 against 449.0) |
| 2. `mut first: Attempt` | Checker gap. Destructured form required. The `mut` form saves 7.1% of the score, below 15%, with one point |
| 3. `RelayUrl::from_str` | Not friction. `.parse::<RelayUrl>()` is the existing form (required fix 3) |
| 4. `.collect::<Vec<LinkState>>()` | The A8 form by design. One point, and `pat.type` stays out |
