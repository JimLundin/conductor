# Bootstrap review

The reviewer's pass over `docs/subset/bootstrap-report.txt`, regenerated against subset version 1 after the WebRTC merge. The report lists 104 construct ids and 2,613 sites. Every id has one decision.

- **A.** The current subset already expresses it. The recipe is below.
- **B.** A proposal in `docs/subset/proposals/` (0002 to 0022), each measured or argued on necessity.
- **C.** Rejected. The code changes shape, as described below.

Where an id splits, the table gives the main decision first, then the part that goes elsewhere. For example, `stmt.let.mut` is B inside an actor (0008) and C everywhere else. Every number comes from `subset measure`. The files measured are in `docs/subset/measurements/`, one directory per proposal or recipe, each with a `before.rs` and an `after.rs`. Rerun one with `./target/debug/subset measure docs/subset/measurements/0004/before.rs docs/subset/measurements/0004/after.rs`. 0009 is the one exception: there, `after.rs` holds the inherent-method form the rule prefers.

## Decisions

<!-- 104 ids: A=56 B=42 C=6 -->
| id | count | decision | proposal or recipe |
|---|---|---|---|
| `expr.await` | 352 | B | 0006 |
| `vis.pub` | 272 | B | 0002 |
| `item.fn.async` | 144 | B | 0006 |
| `stmt.let.mut` | 132 | B | 0008; C §C2 outside it |
| `expr.return` | 89 | A | §A2 |
| `expr.assign` | 71 | B | 0008; C §C2 outside it |
| `expr.if` | 71 | A | §A1 |
| `item.fn.receiver.mut` | 63 | B | 0008; C §C2 outside it |
| `pat.type` | 59 | A | §A8 |
| `type.unit` | 58 | B | 0005 |
| `expr.reference.mut` | 57 | B | 0008; C §C2 outside it |
| `attr.error` | 55 | B | 0010 |
| `expr.binary.eq` | 52 | A | §A4 |
| `expr.binary.and` | 51 | A | §A1 |
| `expr.lit.bool` | 48 | C | §C1; B 0021 for external params |
| `expr.if_let` | 44 | A | §A1 |
| `expr.closure` | 39 | A | §A8; B 0017, 0018 |
| `type.lifetime` | 39 | A | §A9; B 0009 in trait signatures |
| `expr.unary.not` | 37 | A | §A1 |
| `expr.unit` | 36 | B | 0005 |
| `type.reference.mut` | 31 | B | 0008; C §C2 outside it |
| `attr.cfg` | 30 | B | 0016 |
| `expr.index` | 30 | A | §A7 |
| `vis.restricted` | 26 | B | 0003 |
| `expr.for` | 25 | A | §A6 |
| `stmt.let.else` | 24 | A | §A2 |
| `expr.array` | 23 | B | 0015 |
| `expr.loop` | 23 | B | 0007 |
| `expr.unary.deref` | 23 | A | §A12 |
| `type.bool` | 22 | C | §C1; B 0019 for clap flags |
| `attr.from` | 21 | B | 0011 |
| `macro.assert` | 20 | C | §C3 |
| `pat.ident.mut` | 20 | B | 0008, 0018; C §C2 outside them |
| `attr.derive.Deserialize` | 19 | B | 0012 |
| `attr.derive.Serialize` | 19 | B | 0012 |
| `item.impl.trait` | 19 | B | 0009; A §A14 for Display, FromStr, From |
| `expr.binary.add` | 17 | A | §A5 |
| `expr.binary.add_assign` | 17 | C | §C2 |
| `expr.match.guard` | 17 | A | §A3 |
| `attr.tokio::test` | 16 | B | 0020 |
| `expr.continue` | 16 | A | §A6 |
| `generics.type_param` | 16 | B | 0013; A §A11 |
| `expr.range` | 15 | A | §A7 |
| `macro.println` | 14 | A | §A13 |
| `type.slice` | 14 | B | 0014 |
| `attr.serde` | 13 | B | 0012 |
| `type.array` | 13 | B | 0015 |
| `attr.arg` | 12 | B | 0019 |
| `attr.derive.thiserror::Error` | 12 | B | 0010 |
| `expr.repeat` | 12 | B | 0015 |
| `expr.while` | 12 | A | §A6 |
| `item.use.glob` | 12 | A | §A10; B 0018 for the Dioxus prelude |
| `macro.json` | 12 | A | §A13 |
| `macro.matches` | 12 | A | §A1 |
| `type.dyn` | 12 | B | 0017; A §A11 for boxed futures |
| `macro.bail` | 11 | A | §A13 |
| `expr.binary.ne` | 10 | A | §A4 |
| `macro.vec` | 10 | A | §A13 |
| `expr.async` | 9 | B | 0006 |
| `expr.let` | 9 | A | §A1 |
| `expr.struct.update` | 9 | B | 0004 |
| `macro.rsx` | 9 | B | 0018 |
| `macro.tokio::select` | 9 | B | 0007 |
| `expr.cast` | 8 | A | §A5 |
| `expr.while_let` | 8 | A | §A6 |
| `item.use.rename` | 8 | A | §A10 |
| `expr.binary.gt` | 7 | A | §A4 |
| `type.impl_trait.arg` | 7 | A | §A11 |
| `type.infer` | 7 | A | §A8 |
| `attr.component` | 6 | B | 0018 |
| `attr.derive.Default` | 5 | A | §A14 |
| `macro.panic` | 5 | C | §C3 |
| `expr.binary.le` | 4 | A | §A4 |
| `generics.lifetime` | 4 | A | §A9; B 0009 in trait impls |
| `macro.std::pin::pin` | 4 | A | §A13 |
| `expr.binary.ge` | 3 | A | §A4 |
| `expr.binary.mul` | 3 | A | §A5 |
| `expr.binary.sub` | 3 | A | §A5 |
| `expr.break` | 3 | B | 0007 |
| `generics.assoc_binding` | 3 | A | §A11 |
| `item.struct.unit` | 3 | A | §A11 |
| `item.type` | 3 | A | §A11 |
| `macro.env` | 3 | B | 0022 |
| `attr.allow` | 2 | B | 0018; A §A11 for too_many_arguments |
| `attr.command` | 2 | B | 0019 |
| `attr.derive.Parser` | 2 | B | 0019 |
| `attr.tokio::main` | 2 | B | 0006 |
| `expr.binary.bit_xor` | 2 | A | §A5 |
| `expr.binary.lt` | 2 | A | §A4 |
| `expr.binary.or` | 2 | A | §A1 |
| `item.impl.type` | 2 | A | §A14 |
| `macro.tokio::try_join` | 2 | A | §A13 |
| `macro.unreachable` | 2 | C | §C3; A §A3 outside tests |
| `pat.reference` | 2 | A | §A12 |
| `attr.derive.Ord` | 1 | A | §A14 |
| `attr.derive.PartialOrd` | 1 | A | §A14 |
| `attr.derive.Subcommand` | 1 | B | 0019 |
| `expr.binary.div` | 1 | A | §A5 |
| `expr.binary.rem` | 1 | A | §A5 |
| `generics.const_param` | 1 | A | §A11 |
| `macro.anyhow::ensure` | 1 | A | §A13 |
| `macro.asset` | 1 | B | 0018 |
| `pat.rest` | 1 | A | §A12 |
| `type.impl_trait.ret` | 1 | A | §A11 |

## A. Rewrites into the current subset

### A1. Conditions become a `match`

Covers `expr.if`, `expr.if_let`, `expr.let` (let chains), `expr.unary.not`, `expr.binary.and`, `expr.binary.or` and `macro.matches`. Match on the value, or on a tuple of values (0001), instead of testing it. A `bool` from std becomes an `Option` with `.then_some(..)` first, and that counts as a friction point (`examples.md` §11).

Before, `crates/agents/src/claude/session.rs`:

```rust
let initialize = session.control(ControlCall::Initialize).await?;
if let Err(UnsupportedVersion { found, supported }) = check_version(initialize) {
    if !opts.allow_untested_version {
        session.shutdown().await?;
        return Err(Error::UnsupportedVersion { found, supported });
    }
    tracing::warn!("running untested claude {found} (supported: {supported})");
}
Ok((session, events_rx))
```

After (with `allow_untested_version: bool` replaced by `versions: VersionPolicy`, §C1):

```rust
let initialize = session.control(ControlCall::Initialize).await?;
match (check_version(initialize), opts.versions) {
    (Ok(()), _) => Ok((session, events_rx)),
    (Err(UnsupportedVersion { found, supported }), VersionPolicy::AllowUntested) => {
        tracing::warn!("running untested claude {found} (supported: {supported})");
        Ok((session, events_rx))
    }
    (Err(UnsupportedVersion { found, supported }), VersionPolicy::SupportedOnly) => {
        session.shutdown().await?;
        Err(Error::UnsupportedVersion { found, supported })
    }
}
```

### A2. Early exits become `?` or the match's value

Covers `expr.return` and `stmt.let.else`. An absent value becomes an error with `ok_or(..)?`. A branch that ends the function becomes an arm whose value is the result.

Before, `crates/agents/src/claude/session.rs`:

```rust
let (Some(stdin), Some(stdout), Some(stderr)) =
    (child.stdin.take(), child.stdout.take(), child.stderr.take())
else {
    return Err(Error::Command("claude stdio was not piped".into()));
};
```

After:

```rust
let ((stdin, stdout), stderr) = child
    .stdin
    .take()
    .zip(child.stdout.take())
    .zip(child.stderr.take())
    .ok_or(Error::Command(String::from("claude stdio was not piped")))?;
```

`child` stays `let mut`: `Child`'s fields are taken through `&mut`, which 0008(b) allows.

### A3. Match guards become nested matches

Covers `expr.match.guard`, and `macro.unreachable` outside tests. Match on the guarded value inside the arm. An `unreachable!` arm means the match was split in two. Join it back into one match. Since version 7, a guard of one comparison or one `matches!` is allowed, with an unguarded arm for the same pattern after it (proposal 0040). The nested match stays the form for a guard against a literal or a variant.

Before, `crates/agents/src/claude/protocol.rs`:

```rust
ClaudeOutput::System(msg) if msg.subtype == SystemSubtype::Init => { … }
```

After:

```rust
ClaudeOutput::System(msg) => match msg.subtype {
    SystemSubtype::Init => { … }
    _ => Inbound::Event(ClaudeEvent::Unknown(raw)),
},
```

Before, `crates/client/src/transcript.rs` handles `TranscriptDelta` in an `if let` with `return`, then has `EventKind::TranscriptDelta { .. } => unreachable!("handled above")` in the match below. After, the match has one `TranscriptDelta` arm that calls `self.append_delta(text)`. The other arms call `self.close_reply()` first.

### A4. Comparisons become `cmp` and literal patterns

Covers `expr.binary.eq`, `ne`, `lt`, `le`, `gt` and `ge`. Compare with a literal by matching on it (`examples.md` §4). Order two values with `cmp` and match the `Ordering` (§3). Equality of two runtime values with no `Ord` was `.eq(..)` with `.then_some(..)`. Since version 7 it is a match guard (proposal 0040), and the lift is flagged.

Before, `crates/client/src/link.rs` `apply`:

```rust
if event.seq <= cursor.seq {
    return Ok(());
}
if event.seq.0 != cursor.seq.0 + 1 {
    return Err(Lost::Gap { after: cursor.seq.0, got: event.seq.0 });
}
```

After:

```rust
match cursor.seq.next().map(|next| event.seq.0.cmp(&next.0)) {
    Some(Ordering::Less) => Ok(Applied::Duplicate),
    Some(Ordering::Equal) => self.advance(event).await,
    Some(Ordering::Greater) | None => Err(Lost::Gap { after: cursor.seq.0, got: event.seq.0 }),
}
```

### A5. Arithmetic and casts become checked methods

Covers `expr.binary.add`, `sub`, `mul`, `div`, `rem` and `bit_xor`, and `expr.cast`. Use `checked_*` or `saturating_*` (`examples.md` §2), `From`/`TryFrom` for widening and narrowing, and `BitXor::bitxor` for the one STUN XOR.

Before, `crates/client/src/link.rs` `backoff`:

```rust
let jitter_ms = random % (half.as_millis() as u64 + 1);
half + Duration::from_millis(jitter_ms)
```

After:

```rust
let span = u64::try_from(half.as_millis()).unwrap_or(u64::MAX).saturating_add(1);
let jitter = Duration::from_millis(random.checked_rem(span).unwrap_or(0));
half.saturating_add(jitter)
```

### A6. Loops become adapters or the 0007 shape

Covers `expr.for`, `expr.while`, `expr.while_let` and `expr.continue`. A loop over a collection becomes an iterator adapter. `continue` becomes a `filter_map` arm that returns `None`. A loop that waits on a channel becomes the 0007 shape. A `for` with `.await` in its body either batches the work into one write, or becomes a stream adapter whose closure body is an async block (0006(b)). Since version 7, a `for` whose body only runs awaited calls with `?` stays a `for`, and is the form for running effects in order (proposal 0041).

Before, `crates/agents/src/claude/session.rs` `stop`:

```rust
for (_, (_, reply)) in self.pending.drain() {
    let _ = reply.send(Err(Error::Exited));
}
```

After:

```rust
self.pending
    .drain()
    .for_each(|(_, (_, reply))| drop(reply.send(Err(Error::Exited))));
```

Before, `crates/daemon/src/host.rs` `run`: `while let Some(msg) = inbox.recv().await { match msg { … } }`. After: the loop in 0008's "after", `let msg = inbox.recv().await.ok_or(Stopped {})?; self.on(msg, &host);`.

### A7. Indexing, ranges and byte parsing

Covers `expr.index` and `expr.range`. Read with `get(..)`, `split_at_checked(n)` and `try_into()`, which return `Option`/`Result`. Measured on `crates/daemon/src/rtc.rs` `mapped_address`, rewritten both ways: allowing ranges inside `.get(a..b)` saves only 5% of tokens over `split_at_checked` (405 against 426), which is under the bar, so ranges stay out. JSON lookups `raw["method"]` become `raw.get("method").and_then(Value::as_str)`. `frame.rs`'s `self.bytes.drain(..n)` becomes `split_off(n)` plus `std::mem::replace`, inside the 0008 loop state that owns the buffer.

Before, `crates/daemon/src/rtc.rs`:

```rust
let kind = u16::from_be_bytes([attrs[0], attrs[1]]);
let len = u16::from_be_bytes([attrs[2], attrs[3]]) as usize;
```

After:

```rust
let (header, body) = attrs.split_at_checked(4)?;
let (head, tail) = header.split_at_checked(2)?;
let kind = u16::from_be_bytes(head.try_into().ok()?);
let len = usize::from(u16::from_be_bytes(tail.try_into().ok()?));
```

The `while attrs.len() >= 4` loop becomes a recursive `fn attribute(attrs: &[u8], plain: Option<SocketAddr>)`. The recursion depth is bounded by the datagram size.

### A8. Local closures become named functions

Covers `expr.closure` (outside 0017/0018), `pat.type` and `type.infer`. A closure bound to a name becomes a `fn` with typed parameters. `let x: T = …` becomes a turbofish (`collect::<Vec<String>>()`), and `Vec<_>` names its element type.

Before, `crates/agents/src/bin/s1.rs`:

```rust
let log = |name: &str| {
    fixtures.as_ref().map(|dir| {
        let path = dir.join(format!("{name}.jsonl"));
        let _ = std::fs::remove_file(&path);
        path
    })
};
```

After:

```rust
fn log_path(fixtures: Option<&Path>, name: &str) -> Option<PathBuf> {
    fixtures.map(|dir| {
        let path = dir.join(format!("{name}.jsonl"));
        drop(std::fs::remove_file(&path));
        path
    })
}
```

### A9. Lifetimes elide or become owned data

Covers `type.lifetime` and `generics.lifetime` outside trait signatures (0009). A function returning a literal takes `&self`, so its lifetime elides. A borrowed message enum (`Outbound<'a>`) becomes owned. It costs one `String` clone per message sent to a CLI.

Before, `crates/agents/src/claude/protocol.rs`:

```rust
pub(crate) fn subtype(self) -> &'static str {
```

After:

```rust
pub(crate) fn subtype(&self) -> &str {
```

### A10. Imports name what they bring in

Covers `item.use.glob` (outside 0018) and `item.use.rename`. `use super::*;` in tests lists its names (`examples.md` §14). `use std::io::Write as _;` becomes `use std::io::Write;`. No file in the report imports two traits with the same name.

Before, `crates/agents/src/claude/session.rs` tests: `use super::*;`. After: `use super::{ClaudeOptions, PermissionMode, SessionId, SettingSource, command};`.

### A11. Type-level shortcuts are spelled out

Covers `item.type`, `item.struct.unit`, `type.impl_trait.arg`, `type.impl_trait.ret`, `generics.assoc_binding`, `generics.const_param`, `type.dyn` for boxed futures, `attr.allow(too_many_arguments)`, and the `ask<T>`/`request<T>` part of `generics.type_param`.

- `pub type Result<T, E = Error>` goes, and signatures write `Result<T, Error>`.
- `pub struct Stopped;` becomes `pub struct Stopped {}`.
- `Pin<Box<dyn Future<Output = T>>>` and `impl Stream<Item = T>` become `n0_future::boxed::BoxFuture<T>` and `BoxStream<T>`, built with `Box::pin`.
- `impl Into<String>` takes `String`.
- `random<const N: usize>()` becomes `getrandom::fill` on a local array at its two call sites.
- A function with too many arguments takes a named struct.
- `ask<T>`/`request<T>` are inlined into each handle method. Measured on `crates/daemon/src/host.rs`'s three methods, the helper saves 9% (192 against 210 tokens), which is under the bar.

Before, `crates/client/src/upgrade.rs`:

```rust
type Attempt = Pin<Box<dyn Future<Output = Result<rtc::Session, String>>>>;
```

After:

```rust
use n0_future::boxed::BoxFuture;
// field: attempt: BoxFuture<Result<rtc::Session, String>>,
```

### A12. Dereferences and reference patterns

Covers `expr.unary.deref`, `pat.reference` and `pat.rest`. Copy values out with `.copied()`, or bind by value. Write `_` for each tuple field instead of `..`. Box contents are moved with a by-value binding (`Process::Running(running) => running.stop()` with `Running` unboxed, or `Box::into_inner` once it is stable).

Before, `crates/agents/src/claude/session.rs`: `builder.setting_sources(sources.iter().map(|&s| s.into()))`. After: `builder.setting_sources(sources.iter().copied().map(SettingSource::to_cli))`.

### A13. Macros with a core equivalent

- `println!` becomes `writeln!(std::io::stdout(), …)?` (an allowed macro on a temporary) or `tracing::info!`.
- `bail!` and `ensure!` become `Err(Error::…)` as a match arm's value.
- `vec![…]` becomes `Vec::from([…])` (0015).
- `tokio::try_join!` becomes `n0_future::future::try_zip(a, b).await`.
- `std::pin::pin!` becomes `Box::pin`.
- `json!` becomes a typed `#[derive(Serialize)]` struct (0012), or `serde_json::from_str` of a raw string in tests.

Before, `crates/agents/src/bin/s1.rs`: `if failures > 0 { bail!("{failures} step(s) failed"); }`. After: `match failed { 0 => Ok(()), failed => Err(anyhow::Error::msg(format!("{failed} step(s) failed"))) }`.

### A14. Derives and trait impls left out of 0009

Covers the `Display`, `FromStr` and `From` part of `item.impl.trait`, plus `item.impl.type`, `attr.derive.Default`, `attr.derive.Ord` and `attr.derive.PartialOrd`. These become inherent methods: `label(&self) -> String`, `parse(&str) -> Result<Self, E>` and `to_cli(self)`. 0009's measurement shows these are 26% shorter than the trait impls. `Default` becomes a `new` constructor that names the initial state. `HostSeq` ordering compares the inner value, `a.0.cmp(&b.0)` (`examples.md` §3).

Before, `crates/agents/src/claude/session.rs`:

```rust
impl From<SettingSource> for claude_codes::SettingSource {
    fn from(source: SettingSource) -> Self {
        match source { … }
    }
}
```

After:

```rust
impl SettingSource {
    pub fn to_cli(self) -> claude_codes::SettingSource {
        match self { … }
    }
}
```

## C. Rejected, with the shape to use instead

### C1. `bool` in our types

Covers `type.bool`, and `expr.lit.bool` outside 0021. `bool` conflicts with named states (quality 1). A two-state field becomes a two-variant enum, named for what it means. The 22 `type.bool` sites include `allow_untested_version` (`VersionPolicy`), `turn_running` (`Turn::{Idle, Running}`), `Deny { interrupt: bool }` (`AfterDeny::{Continue, Interrupt}`), `done: bool` in transcript replies (`Reply::{Streaming, Done}`), `is_webrtc() -> bool` (match on `Transport` itself), `watch::Sender<bool>` (`watch::Sender<ChannelState>`), and the `report(step, ok: bool, …)` closures (`Verdict::{Pass, Fail}`). A `bool` read from a provider's JSON (`is_error`) is converted to an enum where `protocol.rs` parses the line. The exception is clap flag fields, under 0019.

Before, `crates/daemon/src/host.rs`: `turn_running: bool` with `if self.turn_running { return …Rejected… } self.turn_running = true;`. After: `turn: Turn`, and `match self.turn { Turn::Running => CommandResult::Rejected(Rejection::TurnInProgress), Turn::Idle => self.start_turn(text, host) }`.

### C2. Mutation outside an actor loop

Covers `stmt.let.mut`, `expr.assign`, `expr.binary.add_assign`, `pat.ident.mut`, `item.fn.receiver.mut`, `expr.reference.mut` and `type.reference.mut`, wherever 0008 doesn't admit them. Forward data flow (quality 3) rules them out. The shapes to use instead:

- A counter or accumulator becomes a fold or a collected `Vec` of named results.
- A value changed in place becomes a new value with struct update (0004).
- A builder rebinding (`builder = builder.model(m)`) becomes one chain with `Option` handled by a helper.
- A `&mut T` parameter of our type (`failures: &mut u32` in `link.rs`, `t: &mut Turn` in the bins) becomes a returned value. The state either moves into the loop state that owns it (0008) or comes back from the function.

Before, `crates/agents/src/bin/s1.rs`:

```rust
let mut failures = 0;
let mut report = |step: &str, ok: bool, detail: String| {
    println!("{} {step}: {detail}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        failures += 1;
    }
};
```

After:

```rust
let approve = Check::new("1 approve", writes_once(&t), detail)?;
// … one binding per step …
match Vec::from([approve, deny]).iter().filter_map(Check::failed).count() {
    0 => Ok(()),
    failed => Err(anyhow::Error::msg(format!("{failed} step(s) failed"))),
}
```

Measured on a two-step fragment, this shape is 54% longer (251 against 163 tokens). The `Check` type is fixed overhead, and `s1` has nine steps, so the gap narrows in the real file. It is still a cost. The reviewer keeps C because the before form is a `bool`, an `if` and a captured mutable counter, which go against three of the six qualities at once.

### C3. Tests assert on values, not conditions

Covers `macro.assert`, `macro.panic`, `macro.unreachable` in tests, and `matches!` inside `assert!`. Tests compare values (`examples.md` §14). Map the result to something comparable, such as the error message, a variant-name enum or a `Vec` of events, and use `assert_eq!`. A callback that must not be called returns a value the test then checks, instead of panicking.

Before, `crates/agents/tests/replay.rs`:

```rust
match ClaudeSession::spawn(options(claude)).await {
    Err(Error::UnsupportedVersion { found, supported }) => {
        assert_eq!((found.as_str(), supported.as_str()), ("2.2.0", "2.1.316 to 2.1.322"));
    }
    Err(e) => panic!("wrong error: {e}"),
    Ok(_) => panic!("2.2.0 was accepted"),
}
```

After:

```rust
let refused = ClaudeSession::spawn(options(claude)).await.err().map(|e| e.to_string());
assert_eq!(
    refused.as_deref(),
    Some("claude 2.2.0 is not supported (supported: 2.1.316 to 2.1.322); set allow_untested_version to run it anyway")
);
```

Measured on this test and `oversized_header_is_refused_before_reading_the_body`, it is 11% shorter (117 against 132 tokens) and states the whole expected outcome. `assert!(status.success())` becomes `assert_eq!(status.code(), Some(0))`.

## Gaps found outside the report

- **Interior mutability isn't detected.** `subset check` passes `Rc<RefCell<…>>`, `Arc<AtomicBool>` and `AtomicU64` as plain `type.path`. 14 lines use them, in `crates/daemon/src/rtc.rs`, `crates/client/src/platform/rtc.rs` and `crates/client/src/link.rs`. `docs/subset.md` says these "won't" join. The tool should flag the std cell, atomic and lock types by path (`Cell`, `RefCell`, `atomic::*`, `Mutex`, `RwLock`), and the code should move that state into the owning actor (0008) or a channel (0017).
- **`examples.md` §8 and §9 fail the gate.** `&Vec<T>` parameters trip `clippy::ptr_arg` under `-D warnings`. 0014 fixes them.
- **Macro bodies aren't checked.** `rsx!`, `select!` and `tracing` fields can hide any construct. 0007 and 0018 keep the rule-bound code outside macro bodies where possible.

## Suggested decision order

By how much report volume each decision unblocks, with each dependency placed before what needs it:

1. **0006** async (507 sites). Nearly every actor, test and binary.
2. **0002** `pub` (272), together with **0003** `pub(crate)` (26). The two define each other's meaning.
3. **0007** actor loop (35, plus the 20 `while` loops it replaces). 0008 refers to it.
4. **0008** loop-state mutation (374 sites considered). The largest judgment call, because it weakens quality 3 inside actors.
5. **0005** `()` (94).
6. **0010** thiserror (67), then **0011** `#[from]` (21), which needs 0010.
7. **0012** serde derives (51).
8. **0015** arrays (48). The `vec!` rewrite needs it.
9. **0016** platform `cfg` (30).
10. **0018** Dioxus profile (about 25).
11. **0019** clap (17).
12. **0020** async tests (16). Needs 0006.
13. **0014** slices (14). It also fixes `examples.md`.
14. **0017** wasm callbacks (15).
15. **0021** external `bool` literals (11).
16. **0004** struct update (9). Few sites, but `examples.md` §12 and 0008's subset-only comparison depend on it, so the owner may move it up.
17. **0009** library trait impls (8).
18. **0013** serde-bound generics (7).
19. **0022** `env!` (3).
