# Review: rewrite of `crates/proto` (rw-proto)

The reviewer's pass over `f26ab4f` (`git diff main...rv-proto`) and its friction log, `.audit/rw-proto.friction.md`, under subset version 3. The scratch files for every measurement below are in `docs/subset/measurements/0024/` and `docs/subset/measurements/proto-review/`. Rerun any of them with `./target/debug/subset measure <file>`.

## Compliance

`subset check` reports nothing under `crates/proto/`. The workspace still has 1,065 violations, all in crates that haven't been rewritten yet. `cargo test -p proto` passes 10 of 10. The crate measures 3,707 tokens and 599 lines.

Against the six qualities, the code holds. States are named (`Reassembly`, `Remainder`), every decision is a `match`, and no proto function mutates anything outside 0008(b). The review-only rules in `compromises.md` hold for 0012, 0013, 0014, 0015, 0009 and 0023. The exception is 0002.

### Required fixes

All three are 0002: `pub` marks only what another crate uses. Each was checked by searching `crates/` outside proto for the name.

1. `crates/proto/src/frame.rs:17` and `crates/proto/src/lib.rs:18`. No other crate uses `MAX_FRAME_LEN`. Make it `pub(crate)` and drop it from the `pub use`. The `#[error]` message on `FrameError::TooLarge` still reaches it.
2. `crates/proto/src/ticket.rs:299`. No other crate uses `AuthToken::to_text`. Make it `pub(crate)`. This also leaves no public way to print the secret that `Debug` hides.
3. `crates/proto/src/ticket.rs:303`. No other crate uses `AuthToken::parse`. Its only caller is the `Deserialize` impl. Make it `pub(crate)`.

### Advisory, not required

- `frame.rs:41` reports `u64::MAX` as the size when `usize` doesn't fit a `u64`. That arm can't run on any supported target, so the sentinel never reaches anyone.
- `examples.md` §2 gives `HostSeq` a `next()` method. Proto doesn't have one, so `daemon/src/host.rs:180` and `client/src/link.rs:462` will each need `.0.checked_add(1)` when those crates are rewritten. Adding `next()` in proto now gives them one place to call.

## Tool fix

**`expr.array` rejects `&[…]` as a `const` value.** `tools/subset/src/shape.rs:718-724` allows an array literal only when its slot is a call argument, a method argument or `Slot::ConstInit`. In `const X: &[&str] = &["a", "b"]` the literal's slot is the operand of `&`, not `ConstInit`, so it is flagged. A scratch file with that line reports `expr.array` (reproduced under `/tmp/rvp/k`). 0014's own After block has exactly this line (`DEFAULT_STUN_SERVERS`), so this is a checker bug. The fix: pass `ConstInit` through a `&` whose own slot is `ConstInit`.

**The spec also has two forms for a `const` list.** 0014 shows `&[&str] = &[…]` and 0015(b) shows `[Duration; 4] = […]` (`STUN_SCHEDULE`). Both are allowed once the tool is fixed, which breaks quality 6 (one way). I suggest one form: `&[T]` for a `const` list, so no one counts the length by hand. Arrays would stay for fixed-size bytes and for literal lists passed straight to a call. That narrows 0015(b) and needs the owner, so it is not a numbered proposal. `lib.rs:59` should follow whichever form the owner picks.

## Ratchet

| Candidate | Rows | Before | After | Change | Qualities | Outcome |
|---|---|---|---|---|---|---|
| `PartialOrd`/`Ord` on ordered newtypes | 15 | 380 / 41 | 360 / 41 | -5.3% tokens, 6 sites of one kind | all kept | **Proposal 0024** |
| `HostSeq::freshness` method (in-subset, `examples.md` §3) | 15 | 380 / 41 | 421 / 55 | +11% | all kept | Worse than doing nothing, so 0024 is preferred |
| `&mut self` on `FrameBuffer` | 5, 7 | 581 / 82 | 448 / 66 | -23% tokens | forward data flow weakened | Not proposed. Escalated, see below |
| Const arithmetic | 1 | 16 / 2 | 22 / 2 | +38% | kept | Not proposed. It adds tokens, has 2 sites, and §A5 decided operators |
| Test tables, `Vec::from([…]).into_iter().for_each` | 9, 10 | 181 / 45 | 182 / 40 | +0.6% | kept | Not proposed. No new construct, no saving |
| Test tables, compare two lists in one `assert_eq!` | 9, 10 | 181 / 45 | 159 / 37 | -12% | kept | Not proposed. Under 15%, needs no new construct, and the list's `.map` sits in a macro body the checker doesn't read |

Token and line counts are `subset measure` on the files under `docs/subset/measurements/`. The FrameBuffer "after" compiles: it was built with `rustc` on a scratch file against stubs, and it returned the frame body and `PartialFrame`. The test-table "after" files weren't compiled.

### `&mut self` on a buffer outside an actor

Measured over `FrameBuffer` and both `ChannelRecv::read_body` callers, `&mut self` cuts 23% of tokens and removes both `std::mem::replace` swaps and the `Reassembly` enum. It passes the volume bar. It doesn't keep forward data flow: it widens the one compromise 0008 made, from an actor's loop state to any of our types held behind `&mut`. The pair loop admits only candidates that keep all six qualities, so it is not proposed.

The owner should still see it, for two reasons.

- The bootstrap review's §A7 says `drain` becomes `split_off` plus `std::mem::replace` "inside the 0008 loop state that owns the buffer". It assumed the buffer would live in loop state. Under v3 neither `FrameBuffer` nor `ChannelRecv` is loop state, so the implementer's value-returning design is the faithful reading of the rule as the checker enforces it.
- The cost grows. `subset check` reports 30 more `&mut self` methods outside actor loop state in `client` and `daemon`, including `ChannelRecv::read_body` in both crates and the `read`, `write` and `finish` methods in both `wire.rs` files. Those are the same kind: owned wrappers around a library's `&mut` stream.

I recommend deciding 0008's scope for owned I/O wrappers in the review of the `client`/`daemon` rewrite, where the whole volume is measurable. Until then, `FrameBuffer` stays as written.

## Friction rows

| # | Outcome |
|---|---|
| 1 | Not proposed. Const arithmetic adds tokens (16 to 22) over 2 sites, and §A5 already sends operators to `checked_*`. The doc comment carries the size. |
| 2 | Not proposed. Already decided: `<=` and `ok().filter(…)` on a comparison are operators or a hidden branch (core, quality 2, §A5). |
| 3 | Not proposed. Already decided by §A5 (`From`/`TryFrom` for widening). `as` can't be limited to widening without type information, and it truncates silently, which weakens errors are values. |
| 4 | Not proposed. Same as 2. |
| 5 | Not proposed. Weakens forward data flow (quality 3). Measured -23%. Escalated above as an 0008 scope question. |
| 6 | Not proposed. Already decided by §A14 (`Default` becomes a named constructor). |
| 7 | Not proposed. Part of 5. `let … else` and indexing are decided by §A2 and §A7. |
| 8 | No outcome needed. Not friction, the subset working as intended. |
| 9 | Not proposed. `for` and `while let` stay out by 0007, and a test-only loop would bring a mutable accumulator. The test-table measurements show no saving worth a construct. |
| 10 | Not proposed. The in-subset list comparison saves 12%, under the bar, and needs no new construct. |
| 11 | Not proposed. Already decided by §C3 (tests compare values). |
| 12 | Not proposed. Already decided by §A5 (`as`) and §A7 (indexing and ranges). |
| 13 | Not proposed. Too small: one site, and `==` is decided by §A4. |
| 14 | Not proposed. Already decided by 0009 and §A9. The three `.as_str()` call sites are the cost 0009 accepted. |
| 15 | **Covered by proposal 0024.** The workspace has six sites, not five. |
| 16 | Not proposed. Already decided by 0010 and §A14 (`label(&self) -> String` for a value that isn't a Rust error). |
| 17 | Not proposed. A checker bug, with a one-way ambiguity between 0014 and 0015(b) for the owner. See "Tool fix". |
| 18 | Not proposed. Already decided by 0009. Its rule names `#[arg(value_parser = Ticket::parse)]`, so clap's `value_parser` isn't new evidence. Proto got 5% to 12% shorter. |
| 19 | Not proposed. Too small: one site, and `!=` with an early `return` are decided by §A2 and §A4. |
| 20 | Not proposed. Already decided by §A12 (copy out with `.copied()` or a by-value binding). Too small as well: three sites at 2 tokens each. |
| 21 | Not proposed. Test-only, and decided by §A7 (indexing) and 0008 (`let mut` outside loop state). |
| 22 | Not proposed. Same as 21. |
| 23 | No outcome needed. Not friction. The `PartialEq` added to `TicketError` makes the tests stricter. |
| 24 | Not proposed. Already decided by 0009 (`Display` is a plain method, so `collect_str` goes). +4 tokens. |
