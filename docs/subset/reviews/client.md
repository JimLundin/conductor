# Review: rewrite of `crates/client` into subset v4 (rw-client)

The reviewer's pass over `4a29f1f` (`git diff main...rv-client`) and its friction log, `.audit/rw-client.friction.md`, under subset version 4, with quality 7 ("Repetition is data") applied as accepted. The scratch files and patches for every measurement below are in `docs/subset/measurements/client-review/`. `python3 docs/subset/measurements/client-review/measure.py` (from the repository root) reapplies the two 0008 patches to `4a29f1f` and reprints their token and line counts. Given the `subset` binary and `subset.toml` of branch `subset-score` (`2d4e6de`) as arguments, it also prints the weighted scores. Every compiled "after" below was built in a scratch worktree with `cargo clippy -D warnings`, native and `wasm32-unknown-unknown`.

## Compliance

`subset check` reports nothing under `crates/client/`. The workspace has 360 findings, none in client. The crate measures 13584 tokens and 2004 lines. The friction log's snapshots in `.audit/rw-client.friction/v4/` are byte-identical to the tip, and `measure.py` reprints every number in the log.

The code holds qualities 1 to 6. States are named (`Phase`, `Carrier`, `Host`, `Peer`, `ReplyState`, `Flag`, `Liveness`, `Origin`, `Flow`), every two-way decision is a `match`, and mutation stays in loop state and library `&mut` APIs. The review-only rules hold for 0006 (each `async fn` awaits I/O, a channel or a stand-in's `pending`), 0016 (paired `cfg` items, native stand-ins), 0017 (each callback only forwards to channels), 0023 (every boundary `bool` becomes `Flag` or a named enum before it reaches our types, and `Option<()>` from `then_some(())` stays a local), 0024 (`HostSeq` compared with `cmp`; the one `.0.cmp` at `link.rs:668` compares against `after.0 + 1`, which 0024 leaves to a future `HostSeq::next`), 0025 (each `drop` releases a feed or the link early), and 0028 (each predicate body is `matches!`, a comparison, or `!` over a library `bool`).

### Required fixes

1. **0002, `pub` only for other crates.** `crates/client/src/wire.rs:36` (`Transport`), `:43` (`open_bi`), `:57` (`SendHalf`), `:62` (`RecvHalf`), `:68` (`write`), `:84` (`finish`) and `:97` (`read`) are `pub` in the private module `wire`. No other crate uses them. Make them `pub(crate)`. I made this change in a scratch worktree, and it passes clippy on native and wasm32. `WireError` (`wire.rs:18`) stays `pub`: it is the payload of the public `SendError::Wire`, so rustc's `private_interfaces` lint needs it.
2. **Quality 7, repetition is data.** `crates/client/src/transcript.rs:64-87`. `append` builds `Item::Reply { text, state: ReplyState::Streaming }` in three arms that differ only in the text. Decide the text and the item kept in one `match`, then build the reply once:

   ```rust
   fn append(self, text: String) -> Transcript {
       let mut items = self.items;
       let (kept, text) = match items.pop() {
           Some(Item::Reply {
               text: open,
               state: ReplyState::Streaming,
           }) => (None, format!("{open}{text}")),
           last => (last, text),
       };
       items.extend(kept);
       items.push(Item::Reply {
           text,
           state: ReplyState::Streaming,
       });
       Transcript { items }
   }
   ```

   This form is 100 tokens instead of 129 (-22%), and its score is 91.0 instead of 112.5 (`q7-transcript/`).

### Advisory, not required

- **The nine status publishes** in `link.rs` share the scaffold `let current = …; publish(Status { …, ..current })`, but each site changes different fields, so they don't differ "only in values". I measured the data form anyway: a `Change` enum with one `applied` match (`q7-status-change/`) adds 59 tokens to `link.rs`. It is not required. Proposal 0036 removes the part of the scaffold that is the same at every site.
- **Module paths.** Other crates reach `client::platform::bind_endpoint` and `client::transcript::{…}` through `pub mod`, but 0002 says "exported from the crate root". This was already the case on `main`. A root `pub use` would settle it.
- **0026 without a handle.** `negotiate` (`rtc.rs:197-205`) does a oneshot round trip with the signaling actor through a bare `UnboundedSender`, not through an `ask`. 0026 binds only "each actor handle", so this one site passes. If the owner means "every request/reply round trip", the rule's wording needs to say so.
- **A hello with no deadline.** `hello` (`link.rs:760`) waits as long as QUIC does. With a timeout of a few seconds, the Welcome stall below would retry at once instead of after 30 s. That is a mitigation, not a cause.

## 0008: how far `&mut` reaches

The owner deferred this question until `client` and `daemon` had numbers. The implementer's R1 to R7 put the cost of v4's reach at +463 tokens (+13.5%) against the wanted forms. I checked that number in three ways.

- The totals add up: 3437 wanted against 3900 written (R1 to R7, `measure.py`).
- The wanted forms compile. I applied each one to the real code and ran it through clippy. R1, R2, R3 and the `&mut` part of R7, together with `FrameBuffer`, are `0035-endpoints.patch`, clean on native and wasm32 for `client`, `proto` and `daemon`, and `cargo test -p proto` passes. R5 with R6 is `0008-loop-owned.patch`, clean on native and wasm32.
- One baseline is stale. R7's wanted file is written in v3 forms (`match … Ok(()) => {}`, `drop(reply.send(..))`, the `""` match), so its 46-token residue mixes 0008 with the v4 forms. On the real v4 code, the `&mut` part of R7 is only `Signaling::send` and `stop`. It is counted inside (c) below.

The agents review's (a) (`s2` turn records, -15.8%) is already done under v4, in `836b7f4` on `rv-v4fix`. A `HashMap<_, Turn>` field counts as "owned by loop state". A `Phase::Connected(Connected)` payload does not, so the checker is not consistent here.

In the table, "affected" counts only the items that each option changes. Scores are from `subset score` (branch `subset-score`), with the new `&mut` sites counted as costly (3.0) instead of outside. Every option keeps qualities 1 and 2 ("named states" and "visible branching"), so the table leaves them out.

| Option | Sites it converts | Affected tokens | Affected score | `client` crate | Forward data flow (3) | Errors (4), local reasoning (5), one way (6), repetition (7) | Checker from syntax |
|---|---|---|---|---|---|---|---|
| (a) v4 as is | none | 0 | 0 | 13584 | as now | as now | yes (today) |
| (b) reach through enum payloads, receivers only | none without mixing styles. `Upgrade::retry_later` and `succeeded` are called on a `Connected` held by value (`link.rs:570`, `:580`, `:615`, `:633`), so only `arm` could flip (inferred, not measured) | ~0 | ~0 | ~13584 | as now | one way at risk if `Upgrade` mixes `self` and `&mut self` | yes: follow variant payload types one step, as it already does for generic arguments |
| (b+) (b) plus a `&mut` parameter of a loop-owned value in loop-state methods | R5, R6 | 1389 to 1282, **-7.7%** | -3.6% | 13477 (-0.8%) | weakened further: a `Link` method changes a `Connected` that it was handed. This is the `&mut` parameter that v4 kept out | 5 kept (the `&mut` is in the signature). 4, 6 and 7 kept | yes, with 11 findings to permit: 3 `&mut Connected` parameters, 5 assignments through them, 3 `Upgrade` receivers |
| (c) `&mut self` on stream endpoints, anywhere (**proposal 0035**) | R1, R2, R3, R7's `send` and `stop`, `FrameBuffer` | client 1501 to 1172, **-21.9%**. `FrameBuffer` 221 to 180, **-18.6%** | -19.5%. `FrameBuffer` -11.4% | 13256 (-2.4%) | weakened only where it was already a fiction: a stream's state lives outside the program, and 0008(b) already accepts `&mut` for the library value inside | 4 helped (a failed write no longer drops the half). 5 kept. 6 helped (one idiom for every stream). 7 helped (the hand-back arms go) | yes: a name list in `subset.toml`, like 0009's traits. The patch yields exactly 9 findings, all on the listed types |
| (b+) and (c) | R1 to R3, R5 to R7, `FrameBuffer` | | | 13149 (-3.2%) | both of the above | as above | both of the above |
| (d1) `&mut self` on any type of ours, `&mut` parameters still out | (c), plus R4 `Transcript` and R5 `Upgrade` | (c), plus R4's 517 to 452, -12.6% | (c), plus R4's -7.4% | ~13190 (inferred: (c) plus R4) | weakened everywhere: "values are built" becomes "values change through their methods" | 6 weakened: `fn x(self) -> Self` against `fn x(&mut self)` becomes a matter of taste, with nothing in the code to decide it | yes, trivially |

**Recommendation: (c), as proposal 0035.** It is the only option that passes the 15% bar on the code it touches, by tokens and by weighted score. It removes the four friction points of one kind that cost the most (R1 alone is +56%). It closes the proto review's escalated `FrameBuffer` row. It also gives stream I/O one idiom, where today there are two (library streams by `&mut`, ours by value). The name list keeps quality 6: an endpoint takes `&mut self`, and a data type keeps its value-returning methods. The checker can enforce it from a list, as it does for 0009's traits and 0026's `ask`.

Reject (b+). The affected code shrinks by 7.7%, but its weighted score shrinks by only 3.6%, because it trades neutral struct updates for costly mutation. It also reopens the `&mut` parameter that v4 kept out. Reject (d1): it buys R4's 65 tokens with quality 6.

(b) by itself saves nothing in this crate. It still fixes an inconsistency: a value in a map counts as owned by loop state, and the same value in an enum payload does not. The owner may want the checker to follow variant payloads for that reason alone. It isn't a proposal, since it has no measured gain.

## Ratchet

| Candidate | Rows | Before | After | Change | Qualities | Outcome |
|---|---|---|---|---|---|---|
| `&mut self` on stream endpoints | R1, R2, R3, R7, proto row 5 | 1501 + 221 tokens | 1172 + 180 | -21.9% and -18.6%. Score -19.5% and -11.4% | 3 weakened only for streams, 4 and 6 helped | **Proposal 0035** (costly) |
| Struct update with a method-call base | S1 (9 sites), J1b | 4899 (`link.rs`) | 4854 | -45 tokens, 9 sites of one kind. Score -0.9% | all kept | **Proposal 0036** (neutral) |
| `rfind` and `wait_for` as predicate adapters | B2 (4 sites), the `rfind` from new friction | 161 | 158 | -3 tokens. Score +2.4% (tolerated `==` replaces neutral `.eq`) | 6 helped | **Proposal 0037** (tolerated). It pays in one way, not volume |
| `if let None = x` for "act only when `None`" | new friction: `adopt`, headless `went_live` | 193 | 175 | -9.3% tokens. Score +12% (tolerated `if let`) | 6 at risk | Not proposed. 2 sites, and clippy's `redundant_pattern_matching` rejects `if let None` (and `if let Some(_)`), so it would need a lint switched off, which 0025 revised rules out. The `match` with a value arm is already clippy-clean |
| `ask` optional with one caller | new friction, L7 | 339 | 387 | +48, one site | | Not proposed. Making `ask` depend on the number of callers gives two forms for one task, against quality 6. A second request kind pays it back |
| `drop(x.field)` as a named drop | V4 | 382 | 372 | -10, 2 sites | kept | Not proposed. Too small, and under (b+) both sites assign the field instead. It could be a checker clarification of 0025 |
| Status changes as a `Change` table | S1, quality 7 | 4899 | 4958 | +59 | kept | Not proposed. It adds tokens, and the sites differ in shape, not only in values |

## Friction rows

| # | Outcome |
|---|---|
| R1, R2, R3 | Proposal 0035. |
| R4 | Not proposed. Only (d1) reaches it, and (d1) weakens quality 6. -12.6% on its own, below the bar. |
| R5, R6 | Not proposed. Measured as (b+): -7.7% tokens, -3.6% score. It reopens `&mut` parameters. |
| R7 | The `send`/`stop` residue is in proposal 0035. The rest was done in v4. |
| CL1 | Not proposed. The cost (+23%) is §A12 (`*boxed`) meeting clippy's `large_enum_variant`. The smaller `Connected` is a better design anyway. |
| CL2 | None needed. `empty` is the proto precedent. |
| CL3 | Checker or brief: `#[expect]` is not in the subset, so the brief's fallback can't be used. The owner's lint sweep should settle it. |
| K1, K2 | Not proposed. The stand-ins cost +1%. K2's placeholder returns go away under 0035 (`read_body` becomes a plain `pending().await`). |
| A1 to A4 | Not proposed. The cost is 0007's actor shape, already decided. A2's `Outbox` split is a `Sync` consequence, not a subset gap. |
| F1 | Not proposed. Already decided by 0017. |
| F2 | Checker gap: 0017's own block form `Closure::new({ … })` is flagged. Not a construct question. |
| F3 | Done in v4 (0028). `Flag` stays, correctly (0023). |
| B1 | Done in v4 (0027). |
| B2 | Proposal 0037. |
| B3 | Not proposed. +7 tokens, one site, and `is_selected` is a library `bool` in a predicate, which is allowed. |
| B4, B6 | Done in v4 (0027, 0028, 0024). |
| B5 | Intended (§C1). |
| O1, O4 | Not proposed. Already decided by §A5. |
| O2 | Done in v4, except `link.rs:668`, which waits for `HostSeq::next` (proto review, advisory). |
| O3 | A saving (-10%). |
| O5 | Done in v4 (0027). |
| S1 | Proposal 0036. |
| S2, S3, S4, S5 | Not proposed. Already decided by §A11, §A9, 0010 and §A14. S4's doubled message under `{:#}` is worth a look by whoever owns 0010. |
| New: 0008 reach stops at enum payloads | The decision table above. |
| New: 0026 with one caller | Not proposed (ratchet table). |
| New: 0028 adapter list | Proposal 0037. |
| New: 0027 has no "only when `None`" | Not proposed (ratchet table). |
| New: named drop of a field | Not proposed (ratchet table). |
| New: brief, s3 runs, proto's `DEFAULT_STUN_SERVERS` | Not subset questions. The s3 flake is the Welcome stall below. |

## The Welcome stall

**Symptom (from the friction log).** A fresh connection, mostly the first one after a reload, sometimes never gets the host's `Welcome`. The host logs "client connected" (`crates/daemon/src/server.rs:66`), which it does only after writing and finishing the reply (`server.rs:108-109`). The client then fails with "stream read failed: connection lost" when QUIC's 30 s idle timeout fires, and the next dial works. It happens before and after the conversion, about 1 run in 10.

**Hypothesis (a guess from reading the code, not reproduced).** The two endpoints spell the same relay server two ways, so iroh holds two relay connections to one server under one endpoint id, and the relay delivers each datagram to only one of them.

- The browser strips the root dot from its own relay map (`crates/client/src/platform/web.rs:7-11`) and from the ticket's relay (`crates/client/src/link.rs:49-50`). The daemon's endpoint (`crates/daemon/src/main.rs:109`, `presets::N0`) keeps n0's dotted names, and its address lookup publishes the host's home relay with the dot. The browser's endpoint also uses that lookup (`web.rs:12`). The friction log's WebKit failure on `wss://euc1-1.relay.n0.iroh.link./relay` shows that the browser does dial the dotted spelling.
- iroh treats `…iroh.link` and `…iroh.link.` as two relays. So at least one side opens a second connection, under the same endpoint id, to the server that its home relay already reaches.
- iroh-relay 1.4.1 keeps one active connection per endpoint id (`server/clients.rs:81-93`). A newer connection replaces the active one. The old one is marked inactive, is sent `SameEndpointIdConnected`, and receives no more datagrams.
- When the second connection comes up in the middle of the handshake, datagrams for that endpoint go to whichever connection registered last. If the client's QUIC path to the host is bound to the other one, the `Welcome` and every retransmission are lost, and the connection idles out. Whether this happens depends on when address lookup resolves relative to the dial. That would explain why only some dials fail, and why the retry, made with both connections already up, gets through.

**How to confirm it.**

1. Log a failing run at `iroh=debug,iroh_relay=debug`: the daemon's log, and the browser console with iroh tracing at debug. If the hypothesis holds, the failing dial shows both spellings of the same relay on one side, and a `SameEndpointIdConnected` health message just before the stall. A passing dial shows only one spelling, or the second connection settling before the `Hello`.
2. Run an A/B test. Give both ends one spelling, either dotless relay maps on the daemon too or the dotted map in Chromium, which accepts it. Then run the s3 Upgrade scenario 40 times, interleaved with the current build. At the implementer's base rate of about 1 in 10, the current build should fail about 4 times, and the fixed one none. I saw a process in worktree `conductor-relayfix` building the daemon with `RelayMode::Custom(client::platform::n0_relays())`, which is this experiment. Its result decides the question.

If both ends use one spelling and the stall persists, the hypothesis is wrong. The next suspect would be the hello stream itself (`link.rs:760-769`, `server.rs:88-114`). That stream already holds its send half open until the reply arrives, so I don't expect a cause there.
