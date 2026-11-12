# Review: remove WebRTC (`rv-remove-webrtc` at `61a40ee`)

Pair loop step 2. The diff is `git diff main rv-remove-webrtc` (main is
`0b84736`). The friction log is empty. Token counts come from
`tools/subset measure` and scores from `tools/subset score`, run on scratch
copies under `/tmp`. The checks on edited configs ran the pinned checker
binary directly on those copies.

## Verdict

**Changes requested.** The single-transport link is correct. Reconnect with
backoff, the hedged dial, catch-up from the cursor and the epoch check keep
their code paths and their daemon e2e tests. `tools/subset check` is clean.
Five required fixes remain: four for volume and one for the docs.

## Correctness

- **Lost behaviour: none found.** `dial_hedged`, `backoff`, `adopt`, `apply`
  and `catch_up` are unchanged. These e2e tests still cover them:
  `reconnect_mid_feed_resumes_after_the_cursor_without_gaps_or_duplicates`,
  `a_new_client_resumes_from_a_dead_clients_cursor`,
  `a_cursor_from_another_epoch_starts_over_from_the_beginning` and
  `a_stalled_dial_goes_live_through_a_fresh_endpoint_and_stops_cleanly`. Every
  removed test was WebRTC-only.
- **Error messages.** Every message is kept. On the client, every
  `conn.open_bi()` maps through `WireError::from`, so it still says "could not
  open a stream" (`c5f170d`). On the daemon, "could not finish the stream"
  moved from `WireError::Finish` to `StreamError::Finish` with the same text.
  The only messages removed are WebRTC-only: `WireError::Channel`, the
  `upgrade_error` strings, and the fallback reasons.
- **Stream lifetimes.** `hello` and `rpc` keep `send` alive until their read
  returns, as the old `_send` bindings did. `subscribe` gives both halves to
  `frames`.
- **Wire compatibility.** `Open::Rtc` was the last variant, and `ALPN` moves
  from `conductor/4` to `conductor/5`. That is correct.
- **Dead states.** `Carrier`, `Upgrade`, `Input::Dropped`/`Upgraded`,
  `Status::upgrade_error`, `LinkOptions::stun_servers` and `PathKind::WebRtc`
  are all gone. The only state the upgrade leaves behind is `PathKind` itself.
  It is dead in production (fix 1).
- **Not from this change, optional.** `Lost::Connection(ConnectionError)` is
  not built anywhere in `link.rs`, here or on main. Every `ConnectionError`
  goes through `WireError::Open`. `#[from]` hides this from the dead-code lint.
- **Coverage note, not required.** The browser e2e no longer has a scenario
  where a wasm client resubscribes mid-turn. The old `drop` scenario was the
  only one, and it was triggered by WebRTC. Native e2e still covers
  resubscribing.

## Required fixes

1. **Remove the path detail (`PathKind`, `Status::path`).** A browser can't
   send UDP, so its selected path is only ever `Unknown` or `Relayed`. The
   only time `Direct` is seen is the native harness's assertion in
   `crates/daemon/tests/e2e.rs:259`. The status line shows "Relayed" to every
   user on every connection, which tells them nothing. Remove the enum, the
   field, `Connected::paths`, `Input::PathsChanged` and its arms, and
   `path_kind`. In the app, remove the `match`, `data-path`, `span.path` and
   the `path` log field. Also remove the re-export in `client/src/lib.rs`,
   the e2e assertion, `.status .path` in `main.css`, and in `browser.mjs` the
   `path()` helper with its two `Relayed` checks, the `:not([data-path=
   "Unknown"])` wait, and the screenshot `style` that hides the path.
   Measured on scratch copies: `link.rs` goes from 4824 to 4591 tokens (-233,
   -33 lines, score -198). `app/src/main.rs` goes from 1700 to 1628 (-72, -7
   lines, score -111.5). That is about -305 Rust tokens before the test, CSS
   and JS lines. The edited files pass `subset check`.
2. **Share one `wire.rs`.** The two files have the same `read` and `write`.
   The only differences are two error variants and `pub`. Proposal 0013 rules
   out a generic error type: a codec fn may have one `Serialize` or
   `DeserializeOwned` bound and nothing else. `#[path]` and `include!` are not
   in the subset. So share the error type as well:
   - In the client, make the module `pub mod wire` and make `read` and `write`
     `pub`.
   - Move `client` from the daemon's `[dev-dependencies]` to its
     `[dependencies]`. Every native dependency of `client` is already a
     dependency of the daemon.
   - Delete `crates/daemon/src/wire.rs` and its `mod wire;`, and import
     `client::wire::{self, WireError}` in `serve.rs`.

   `proto` does no I/O. Saving: -275 tokens and -29 lines from the daemon's
   copy, and about -4 more from `pub(crate)` becoming `pub`. The daemon then
   has `WireError::Finish` and `Open` variants that it never builds, but
   `StreamError::Finish` still handles its finishes. If the owner reads the
   dependency from daemon to client as a layering breach, use a `crates/wire`
   crate instead. It saves the same Rust tokens, but it adds a `Cargo.toml`
   and an entry in the `WASM_CRATES` list in `tools/check.sh`.
3. **Inline `spawn_stream` into `streams`, its only caller.** It had two
   callers before, the iroh accept loop and `channels`. Measured: `serve.rs`
   goes from 1447 to 1418 tokens (-29, -9 lines, score -37.5), and the edit
   passes `subset check`. The arm becomes:
   ```rust
   Ok((send, recv)) => {
       let stream = self.clone().stream(conn.clone(), send, recv, client.clone());
       tokio::spawn(async move {
           if let Err(err) = stream.await {
               tracing::debug!(%err, "stream ended with an error");
           }
       });
   }
   ```
4. **`subset.toml`: drop the unused `type.dyn` scope and `endpoints = []`.**
   - Nothing in `crates/client/src/platform/` uses `dyn` now. The scope still
     admits `Closure<dyn FnMut()>` there. Failing input:
     `pub fn f(_x: wasm_bindgen::closure::Closure<dyn FnMut()>) {}` in
     `platform/web.rs` passes `check` today. Without the scope it fails with
     `type.dyn: trait object`.
   - The checker reads `endpoints` with `#[serde(default)]`
     (`src/main.rs:52` at the pinned revision), so `endpoints = []` means the
     same as no key. Delete it and its three comment lines.
   - Proposals 0017 and 0035 stay accepted in the subset repository. Adding
     either back takes one entry.
   - Checked: the worktree passes `check` with the trimmed config.
5. **Docs: every connection is relayed, so say that.** `docs/architecture.md:20`
   says "through n0's public relays where a direct path isn't possible". The
   only client is the browser, so a direct path is never possible. That
   contradicts D3 ("in a browser every iroh connection is relayed") and "There
   is no native client". `docs/roadmap.md:13` has the same clause, and its
   "the browser included" suggests that other clients exist. Write "Clients
   reach hosts over iroh through n0's public relays (§4)", and the same in
   the roadmap.

## Suggested, not required

- D3b's rationale said that n0's relays "are meant for development". That
  risk is now the whole transport, and the docs no longer say so. Record it
  as a known limitation in the roadmap, next to D3's "the relay map is one
  setting".
- `docs/p1/smoke.md`'s "Historical" note is correct. The smoke steps and the
  rest of both docs have no WebRTC left. A `git grep` for WebRTC, STUN,
  str0m, `rtc`, upgrade, transport and carrier in code, CSS, JS, tools and
  fixtures finds only Codex's model `"upgrade"` fields.

## The screenshots

**This is a test-setup artifact. Fix the e2e.** Showing "Thinking…" to a
client that joins mid-reply is correct current behaviour, because deltas are
live-only. But the `running` shots exist to show the streaming reply, which
every live viewer sees. The main page checks it (`.reply.streaming` with
`/tmp/s1`), and the committed images no longer show it.

The fix is in the `screens` scenario. Open the four shooter contexts first,
wait for each to be Live, and only then tap Approve. Then take the shots
from those contexts. The recording replies only after Approve, so every
shooter receives the deltas live, and `held()` needs no change. The
late-join view does not need a shot.

## Compliance

`tools/subset check` on the branch: exit 0. `tools/check.sh` on the branch:
exit 0 (fmt, the IP scrub, clippy native and wasm, 81 tests in 15 suites, subset).
