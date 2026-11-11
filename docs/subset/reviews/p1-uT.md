# Review: unit T, the WebRTC transport in subset v5

The reviewer's pass over `6036d51` (`git diff main...rv-p1-uT`, two commits) and the implementer's 16 friction points in `.audit/p1-uT.friction.md`, under subset version 5. The contract is `docs/p1/design.md` §5, unit T: `wire.rs`, `rtc.rs` and `stun.rs` in the subset, with the same behaviour. The scratch files for every measurement are in `docs/subset/measurements/p1-uT/`. From the repository root, `docs/subset/measurements/p1-uT/measure.sh` reprints every score and token count below. All scores take the three files together, because `subset score` on `stun.rs` alone treats `Stun`'s `&mut self` methods as outside the subset (see the tool notes).

## Compliance

`subset check` reports nothing in `crates/daemon/src/{wire,rtc,stun}.rs`. The remaining daemon findings are in `host.rs`, `server.rs`, `lib.rs`, `main.rs` and `tests/e2e.rs`, which later units replace. The three files measure 8,236 tokens and 1,128 lines, against 5,512 and 792 on `main` (+49.4%). The weighted score is 7,809.5, against 7,072.0 on `main` (+10.4%).

The code holds the seven qualities in most places. States are named (`Phase`, `Knob`, `Fate`, `Progress`, `ReadStep`, `Ended`), the atomics are gone, mutation stays in the driver's loop state and in library `&mut` APIs, and the review-only rules hold for 0006 (each `async fn` does I/O or awaits a channel), 0014 (`&[String]`, `&[Socket]` and `&[u8]` are read-only parameters), 0015 (`[u8; 12]` is the STUN transaction ID), 0023 (the clap `bool` becomes `Option<Blackhole>` in `main`), 0025 (`drop(open)` in `Driver::close` releases the channel's writers) and 0036 (`..probe.clone()` returns a `Probe`).

The verdict is **not yet compliant on volume**. Three required fixes follow. None changes behaviour. All three are in `required-fixes.diff`. With them applied in a scratch worktree, `cargo clippy -p daemon --all-targets -- -D warnings`, `cargo fmt --check`, `subset check` and the 8 daemon unit tests (including the three STUN parser tests) pass. The three files go from 8,236 to 7,753 tokens (-5.9%) and from a score of 7,809.5 to 7,391.0 (-5.4%). Against `main` that is +40.7% in tokens and +4.5% in score, instead of +49.4% and +10.4%. The design budgeted 6,900 tokens, so the unit is still about 850 over. The rest of the growth is accounted for under "Budget".

### Required fixes

1. **STUN parsing follows the bootstrap review's A7 form, a recursive `fn` over `split_at_checked`.** `stun.rs:248-345` reads the attributes with `std::iter::successors`, an `Attribute` that owns a `Vec<u8>` copy of the rest of the message, a `Value` enum, a `try_fold` over `ControlFlow`, and an `ipv4_at` with three offset reads. `docs/subset/bootstrap-review.md` §A7 already gives the subset form of this loop: `fn mapped_address(attributes: &[u8], plain: Option<SocketAddr>)`, which splits off the 4-byte header, splits the value with `?` (a value cut short rejects the whole response), takes the rest with `split_at_checked(..).unwrap_or_default()`, and matches `(kind, ipv4(value))` to return, recurse with `Some(addr)`, or recurse with `plain`. The depth is bounded by the 2,048-byte datagram. `binding_response` splits the 20-byte header once, so `mapped_address` needs no offset. `Attribute`, `Value`, `attribute`, `Attribute::next`, `Attribute::read`, the `ControlFlow` import and the `Ordering` import go. The parser with its constants goes from 849 to 571 tokens (-32.7%) and from a score of 788.5 to 550.0 (-30.2%). The file total drops by 290 tokens and 250.5 score. The three STUN tests pass unchanged, including the cut-short case.
2. **The driver's turn is one method, so `Wake`, its `Box` and the clone go.** `rtc.rs:438-442` defines `Wake { Transmit, Event(Box<Event>), Input { .. } }` only so that `next` can hand str0m's output to `on`. Clippy's `large_enum_variant` then forces the `Box`, and because the subset has no `*`, `event` takes `&Event` and clones every inbound data-channel message (`rtc.rs:654`). Merge `next` and `on` into `async fn turn(&mut self) -> Result<(), Ended>`, which matches `(self.rtc.poll_output(), self.phase)` and handles each output in its arm: transmit it, pass the `Event` by value to `event`, or `wait` for an `Input` (now `wait` returns `Result<Input, Ended>`), handle it and run the timeout. `run` becomes `match self.turn().await { Ok(()) => {} Err(ended) => break self.end(ended) }`, the drive shape that `ChannelRecv::read_body` already uses. `subset check` accepts it under 0007, and the turn order is unchanged. This is -122 tokens and -97.0 score, and it removes friction T4 with no new construct.
3. **Two helpers that only add a layer.** `millis` (`rtc.rs:870-872`) narrows `as_millis()` to `u64` for two log fields, but `tracing` records `u128` directly (`tracing-core` 0.1.36, `Visit::record_u128`). Log `self.started.elapsed().as_millis()` and delete it. `forward_signal` (`rtc.rs:402-414`) is the body of `read_signals`'s loop moved into a second function, while `read_socket` four lines up keeps its `match` inline. Inline it, with `Ok(None) => break Err(Stopped {})` and the error arm logging before `break Err(Stopped {})`. Together that is -71 tokens and -71.0 score. This removes friction T9.

### Advisory, not required

- `Probe` holds `socket: Arc<UdpSocket>` and `local: SocketAddr`, which is a `Socket`. Holding the `Socket` saves about 9 tokens.
- `Probe::tick` and `Probe::send` could be one `match`, saving a function header (about 12 tokens). The split reads well, so it stays.

## Same behaviour

The commit message names three structural changes: the one actor loop with a `Dropping` phase, the atomics replaced by watch senders and a driver-side frame count, and the offset readers in STUN. Reading old and new side by side, these further differences exist. None is caught by a test, and none matters to the three browser scenarios as far as reading `tools/s3-browser.sh` shows (inferred, not run).

1. **A write after `finish` on a data channel.** The old `ChannelSend::close` cleared the shared open flag at once, so a later `write` on the same half returned `Closed`. Now `write` succeeds until the driver has handled `Out::Close` and dropped the watch sender. That frame is then queued behind the close. `server.rs` never writes after `finish`, so nothing reaches this today.
2. **`Out::Close` is sent twice.** `close()` and then `Drop` each send one. The driver ignores the second, because `writable.remove` already returned `None`. The old code sent one.
3. **Frames are counted when the driver takes them.** The old count rose when a writer queued a frame's chunks. The drop knob still fires on the same frame, because the queue is FIFO. The `frames_out` field of "webrtc session ended" no longer counts frames still in the queue.
4. **Two log lines changed.** "webrtc offer accepted" now prints `blackhole=None` or `blackhole=Some(Blackhole)` instead of `false` or `true` (`docs/s3/browser-run.txt:82` still shows `blackhole=false`). Its `stun=` list no longer drops a server listed twice, because the old code deduplicated names built per probe.
5. **STUN requests go out in `HashMap` order** rather than server by server. The schedule and the give-up time are unchanged.
6. **`WireError`'s `transparent` became `"{0}"`.** The `Display` text is identical. `source()` now returns the inner error instead of the inner error's source, so an `anyhow` chain printed with `{:#}` would repeat the message. Every current log uses `%err`, so no output changes.
7. **A socket whose `local_addr` fails** is dropped at bind time with a warning. The old code kept it, failed `accept` on its host candidate, and never read it.

Verification is not on record. The design's check for unit T is that `tools/s3-browser.sh` passes all three scenarios. The branch doesn't change `docs/s3/browser-run.txt`, whose line 82 still has the old `blackhole=false`, so that file wasn't regenerated from this code. The run is still needed, on the branch with the required fixes.

## Budget

After the fixes, the unit is about 850 tokens over the design's 6,900. These are the parts that cost the most, and why they stay. Counts are function tokens from `subset score --by fn`. The `Stop` and `ReadStep` rows add about 25 and 40 tokens for those types (estimated).

| Part | `main` | After fixes | Why it stays |
|---|---|---|---|
| Actor loop: `run`, `turn`, `wait`, `connecting_in_time`, `input`, `timeout`, `transmit`, `end` | 692 (`run`, `drive`, `poll`) | 884 | 0007's turn shape, and `Ended` replaces format strings with a typed reason. |
| `flush`, `flushed`, `Stop` | 86 | 270 | No `while let` or `continue`. A recursive `flush` would recurse once per pending chunk, which is unbounded, so the `try_fold` stays. |
| `ChannelRecv::read_body`, `step`, `receive`, `ReadStep` | 157 | 281 | The same shape as `crates/client/src/platform/rtc.rs:635-687`. One way. |
| STUN outside the parser (`resolve`, `lookup`, `Probe`, `send_due`, `answer`) | 570 | 875 | T12 and T13: streams instead of `for` with `.await`, and probes rebuilt by value. |
| STUN tests | 239 | 483 | T16. No indexing or `let mut`, one const per message part, and one more case. |
| STUN parser | 274 | 487 | Fix 1 brought it down from 765. The rest is the offset readers, which the ratchet below measures. |

The design's estimate scaled the client's +33% growth. After the fixes this unit grows +40.7%. The difference is the STUN parser and `flush`, the two places where the subset has no loop over a byte buffer or a queue.

## Friction points

- **T1, str0m outputs as inputs.** Accepted, and simplified by fix 2. The `Phase::Dropping` drain is clearer than the old nested `poll` after `Kill`.
- **T2, `flush` without `while let`.** Accepted. See the Budget table for why recursion is not an option here.
- **T3, `then_some(())` on library `bool`s, three sites.** Three of one kind meets the count, but both candidate constructs weaken a quality. A `true`/`false` pattern decodes a `bool` (quality 1), and an `if` is a branch outside `match` (quality 2). `examples.md` §11 names `then_some` as the form. Not proposed.
- **T4, `Box<Event>` and the clone.** Removed by fix 2. See the ruling below.
- **T5, STUN byte parsing.** Fix 1, then the ratchet below.
- **T6, `BitXor::bitxor`.** See the ruling below.
- **T7, `Count`.** Accepted. `examples.md` §2 makes counters newtypes.
- **T8, atomics to watch senders.** Accepted. It removed `Out::Kill`.
- **T9, `millis`.** Removed by fix 3.
- **T10, `if let Count(0)`.** Accepted. `remote` is set only where the count rises, so the condition is the same.
- **T11, `Stream` and `Write` as named structs.** Required by 0001.
- **T12, `n0_future::stream::iter(..).then(..)`.** Accepted. It is the 0006 form, and `n0-future` is already in the workspace.
- **T13, probes keyed by transaction ID.** Accepted. A `&mut self` method on `Probe` would be two steps below the loop state, which 0008 doesn't reach.
- **T14, `Option<Blackhole>`.** Accepted. Clippy rejects both conversions to a two-variant enum, and `Option` is still a named enum, not a `bool`.
- **T15, `repeat_n` for a zeroed buffer.** Accepted, as in the client.
- **T16, the STUN tests.** Accepted. The four cases in `binding_response_reads_xor_mapped_then_plain` are already one line each over the `mapped` helper, so a table would only add the struct that 0001 requires.

## Ratchet

Every candidate for T5 is measured against the parser after fix 1, because that is the shortest form v5 already allows. Against the branch tip, any candidate would look 30% better for reasons that have nothing to do with the candidate. The affected code is the parser and its constants, 571 tokens with a score of 550.0.

| Candidate | Rule tried | Score | Tokens | Change | Qualities | Outcome |
|---|---|---|---|---|---|---|
| Ranges inside `get(..)`, as in `message.get(4..8)?` | `expr.range`, neutral | 496.5 | 528 | -9.7% | Kept | Not proposed. Under 15%, one friction point. The bootstrap review measured 5% on the old parser. |
| Array patterns, as in `Some([_, FAMILY_IPV4, p0, p1, a, b, c, d])` | `pat.slice`, encouraged | 510.5 | 514 | -7.2% | Repetition weakened: the header pattern spells out the cookie bytes that `MAGIC_COOKIE` already holds. The transaction ID has to be rebuilt as an array literal, which 0015 rejects. | Not proposed |
| A byte-reader endpoint `Reader` with `u16`, `u32` and `take` | `endpoints` entry | 593.5 | 610 | +7.9% | Forward data flow and local reasoning weakened. Without a lifetime the reader copies the rest of the message on every read. | Not proposed. Longer, and it wraps no library stream, so 0035 wouldn't admit it. |
| `bytes::Buf::try_get_u16` on a `mut` slice | Already passes `subset check` in v5 | 441.0 | 452 | -19.8% | Forward data flow and local reasoning weakened. `Buf` for `&[u8]` reassigns the slice on every read, and `(header.try_get_u16(), header.try_get_u16())` gives two different fields only by evaluation order. | Not adopted. See the tool notes. |

**No proposal.** No candidate both clears the bar and keeps all seven qualities, so this review claims no number. That includes 0038. T5 is one friction point, so the "three of one kind" route doesn't apply either. The only candidate over 15%, `bytes::Buf`, is already legal in v5. It is not a construct to propose. It is a gap in the checker to close.

## Rulings

**`BitXor::bitxor` is in the spirit of the rule.** Arithmetic goes through checked methods so that overflow becomes a value the caller has to handle (`examples.md` §2). XOR can't overflow or panic, so the operator `^` hides nothing, and the method form neither gains nor loses anything. `bootstrap-review.md` §A5 already names `BitXor::bitxor` as the form "for the one STUN XOR". It is still the one, in `unxor` (`stun.rs:327-330`). XOR doesn't get a proposal for `^`. Two sites at +2 tokens each is far under the bar, and the method keeps one form for every operation outside `match`.

The same mechanism is a real loophole for the operators that can fail. In a scratch file, `a.add(b)`, `Add::add(a, b)` and `a.shl(70)` all pass `subset check`. They panic in debug builds and wrap in release builds, which is exactly what `checked_add` exists to prevent. **The checker should ban them.** Calling a trait method needs the trait in scope, and the prelude doesn't import the operator traits, so a type-free rule closes the gap. Flag any `use` of, or any path through, `std::ops::{Add, Sub, Mul, Div, Rem, Neg, Shl, Shr}` and their `*Assign` forms, with a message that names `checked_*` or `saturating_*` as the fix. Keep `BitXor`, `BitAnd`, `BitOr` and `Not`, which are total. `examples.md` §2 should say this in one line. This narrows what `check` accepts and adds no construct, so it goes to the owner as a checker change, not as a proposal.

**The `Box` is not needed, and no move out of a `Box` is warranted.** The `Box` exists only because of the `Wake` enum. Fix 2 removes `Wake`, so `Event` is never stored in an enum of ours, `event` takes it by value, and the clone goes. With no remaining site, a proposal for `*boxed` has no evidence. If a large library value ever has to sit in an enum of ours again, that site is the evidence to measure.

## Spec and tool notes

- **Cursor slices pass as library `&mut`.** `let mut header = message;` followed by `header.try_get_u16()` passes 0008(b) and 0023, because the checker approximates "never reassigned" by syntax, and `Buf` reassigns the slice inside the call. `compromises.md`'s "whose type it is" list should name this case. A `mut` binding or parameter of type `&[T]` or `&str` is a cursor, not a library value lent as `&mut`, and the checker can flag a `mut` parameter whose declared type is a slice reference.
- **`subset score` on one file loses cross-file facts.** Scored alone, `stun.rs` counts `Stun::send_due` and `Stun::answer` as outside (`item.fn.receiver.mut`, 5.0), because `Driver`, which owns `Stun`, is in `rtc.rs`. `check` gathers facts from every file under `include`. `score` should do the same, or say in its output that the facts are partial.
- **The design's budget.** The P1 budget took 6,900 tokens for unit T. After the fixes it is about 7,750. The design should carry the measured number.
