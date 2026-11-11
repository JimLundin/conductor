---
construct: toolchain lint config (clippy::single_match, clippy::unnecessary_filter_map, rustc dropping_copy_types)
status: proposed
task: rewrite of crates/agents (rw-agents, friction rows C16, Y6, Y3, S2, Z5)
---

# Let the subset's own forms past three default lints

## Rule
The workspace allows three default lints whose suggested fix is a construct the subset excludes, so subset code can write its one form without padding:

- `clippy::single_match`. Its fix is `if let`, which the subset excludes (§A1). A `match` whose second arm is `None => {}` or `_ => {}` is the subset's form for "act only on this case".
- `clippy::unnecessary_filter_map`. Its fix is `filter` with a `bool` closure, which the subset excludes (`examples.md` §8 says to use `filter_map` with a `match` instead). With the lint allowed, `filter_map(..).count()` is the one way to count matching items.
- rustc's `dropping_copy_types`. Its fix is a second discard idiom (`.ok();` or `let _ =`). With the lint allowed, `drop(..)` is the one way to discard a value we don't need, whether or not its type is `Copy`.

`clippy::obfuscated_if_else` stays on. Its fix (an `if`) is excluded too, but the form it rejects, `b.then_some(x).unwrap_or(y)`, hides a two-way branch in a method chain. The subset's form for that is a `match`, so clippy and the subset agree on what to reject.

The config goes in the root `Cargo.toml`, and every member opts in with `[lints] workspace = true`:

```toml
[workspace.lints.rust]
dropping_copy_types = "allow"

[workspace.lints.clippy]
single_match = "allow"
unnecessary_filter_map = "allow"
```

## Before
Each site as the agents rewrite wrote it to get past the lint.

```rust
// codex/server.rs:410-419. The trace arm exists only to give the match a second arm.
match self.open.remove(&id) {
    Some(_) => {
        self.emit(CodexEvent::Thread { thread, event: ThreadEvent::ApprovalCancelled { id } }).await;
    }
    None => tracing::trace!("codex resolved {}, which we answered", id.0),
}

// codex/server.rs:444-453. The debug arm exists for the same reason.
match (method.as_str(), &outcome) {
    (methods::INITIALIZE, Ok(_)) => match self.write(Outbound::Initialized).await {
        Ok(()) => {}
        Err(e) => tracing::error!("sending initialized: {e}"),
    },
    (methods::INITIALIZE, Err(message)) => {
        tracing::debug!("codex refused initialize: {message}")
    }
    _ => {}
}

// claude/session.rs:477 and codex/server.rs:481. Every other discarded send in the crate is `drop(..)`.
reply.send(self.stop().await).ok();

// bin/s1.rs:540 and bin/s2.rs:593
.fold(0_usize, |failed, verdict| match verdict {
    Verdict::Pass => failed,
    Verdict::Fail => failed.saturating_add(1),
});

// tests/replay.rs:216-222. A third counting idiom.
let results = recording
    .lines()
    .map(|line| match frame_type(line).as_deref() {
        Some("result") => 1,
        _ => 0,
    })
    .sum::<usize>();
```

## After
```rust
match self.open.remove(&id) {
    Some(_) => {
        self.emit(CodexEvent::Thread { thread, event: ThreadEvent::ApprovalCancelled { id } }).await;
    }
    None => {}
}

match (method.as_str(), &outcome) {
    (methods::INITIALIZE, Ok(_)) => match self.write(Outbound::Initialized).await {
        Ok(()) => {}
        Err(e) => tracing::error!("sending initialized: {e}"),
    },
    _ => {}
}

drop(reply.send(self.stop().await));

.filter_map(|verdict| match verdict {
    Verdict::Fail => Some(verdict),
    Verdict::Pass => None,
})
.count();

let results = recording
    .lines()
    .filter_map(|line| match frame_type(line).as_deref() {
        Some("result") => Some(line),
        _ => None,
    })
    .count();
```

The full functions are in `docs/subset/measurements/0025/before.rs` and `after.rs`. The rewrite was applied to a scratch worktree with the config above. `cargo clippy -p agents --all-targets -- -D warnings`, `cargo fmt --check`, `cargo test -p agents` (31 tests) and `subset check` all pass. Without the config, the same rewrite fails clippy with exactly these lints: `single_match` at both `server.rs` matches, `dropping_copy_types` at both `Shutdown` arms, and `unnecessary_filter_map` at the three counting sites. `initialize` failing still reaches the caller as `Error::Rpc`, so the removed `debug!` line carried nothing the caller lacks.

## Measurements
| | tokens | lines |
|---|---|---|
| before | 1139 | 186 |
| after | 1098 | 185 |
| change | -41 (-3.6%) | -1 |

The measured fragment is the whole of each affected function or statement. By lint:

| Lint | Sites | Before | After | Change |
|---|---|---|---|---|
| `single_match` | 2 | 252 | 222 | -11.9% |
| `dropping_copy_types` | 2 | 46 | 44 | -4.3% |
| `unnecessary_filter_map` | 3 | 215 | 206 | -4.2% |

Friction points removed: 5 of one kind, "clippy against the subset" (Y3, C16, Y6, and the counting part of S2 and Z5). The token saving is small. The case rests on the qualities: two log lines that exist only to satisfy a lint, and three idioms for one task each in discarding (`drop`, `.ok()`) and counting (fold, `map` to 1 and `sum`, `filter_map` and `count`).

## Qualities
1. Named states: kept.
2. Visible branching: helped. The two `match`es now show exactly the decision the code makes. The padding arms made it look as if a failed `initialize` and an already-answered request each needed handling.
3. Forward data flow: kept.
4. Errors are values: kept. `drop` of a send result discards the same value `.ok()` did.
5. Local reasoning: helped. A reader no longer asks why a `trace!` is there.
6. One way: helped. One discard idiom (`drop`), and one counting idiom (`filter_map` with a `match`, then `count`), matching §8. Allowing the lints adds no second form, because the forms they would suggest (`if let`, `filter` with a `bool`, `.ok();`) are already outside the subset or become unneeded.

The risk `dropping_copy_types` guards against is `drop(&guard)` meant to release a lock. The subset has no locks (interior mutability stays out), so that mistake has nothing to hit in subset code.

## Owner decision
