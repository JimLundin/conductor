# Verification boundaries

| Layer | What it checks |
| --- | --- |
| Wire encoding and parsing in `claude`, `codex` and `proto` | Provider JSON lines and host/client messages decode to the expected types. Outbound requests and responses encode the fields the peer expects. |
| Adapter replay in `claude-cli` and `codex-cli` | Recorded CLI exchanges exercise process and thread lifecycle, request correlation, interruption, EOF and resume through the adapter. |
| Provider-event mapping in `claude` and `codex` | Provider messages become the intended common event or request, with the IDs, outcomes and details the host uses. |
| Host tests and daemon E2E in `host` and `daemon` | Commands reach the correct session, receipts and events survive retry and restart, and real host links catch up. The daemon tests run with fake or recorded providers. |
| Workspace Git tests and daemon E2E | Initial and turn snapshots record commits, unchanged turns can share a commit, and diffs compare consecutive recorded snapshots even when the agent made several commits. Capture failure and restart preserve pending work and branch commits; retained commits stay reachable. Ownership checks, diff limits, original-checkout isolation, finishing and safe removal remain covered. |
| Client fold in `client` | Event streams, reconnects and snapshots reconstruct session state and the pending actions shown to a client. |
| Browser E2E in `crates/app/tests-e2e` | The web app's layout, browser storage, interaction and event wiring work against the daemon with fake CLIs, including separate turn rows and diffs when commits repeat. |
| Real CLI compatibility | The compiled pins, version checks and recorded exchanges anchor replay to known CLI versions. Re-record provider behavior when changing a pin and document it in the provider's `PROTOCOL.md`. |
| Real host and CLI in `tools/live-host.sh` | The real host can configure a provider, open a session and handle approvals, interruption and Continue over iroh. |

Keep assertions at each boundary where they detect a wiring or behavior
failure. The same user action can exercise distinct guarantees in the wire,
daemon and browser suites.

Historical undo and provider rewind are removed. Current snapshot checks
cover capture and per-turn comparisons; Continue checks still verify
ordinary provider resume after a restart. The dated run and allocation
report below describe the versions tested then, including features since
removed.

## Models in the live-host check

Use `tools/live-host.sh --provider claude --model haiku` or
`tools/live-host.sh --provider codex --model gpt-6-luna` when the selected
instance advertises that model. These calls send the choice with
`StartSession`. All scenarios in the run use the same option, including
the session opened for the crash and Continue check. The Claude
`session3-questions` recording lists `haiku` in `initialize`, and the Codex
`server1` recording lists `gpt-6-luna` in `model/list`.

Without `--model`, the check keeps its cheap host defaults. Explicit
session choices pass through `host::session::choose`, which requires the
exact ID in the instance's current choices. Claude choices are the named
`initialize` rows, excluding the synthetic default, and Codex choices
exclude hidden `model/list` rows. A provider or gateway can accept a model
that its discovery omits. The host default can name that model because
`host::registry::offered` sets the default independently of the choices.
With Codex registration, the stored provider credential names the default
model and takes precedence over the host setting. The real-device smoke
check also uses the host setting to select a cheap model from the browser.

## Registered-provider run on WSL2, 21 November 2026

The live-host driver started five sessions for each registered gateway in
isolated provider homes. Each session's creation event identified the expected
account. Both providers passed file approval, interruption on first output,
interruption at send, the subprocess credential check, and crash followed by
Continue with the original codeword. The source checkouts stayed clean.
One additional session per gateway exercised the optional undo scenario.
Both restored the retained file and conversation history and removed the
discarded file.

| Pinned CLI | Session model recorded by Conductor | Model in the provider's own session record |
| --- | --- | --- |
| Claude Code 2.1.316 | `haiku` | `claude-haiku-5-5` in assistant messages |
| Codex 0.166.0 | `gpt-6-luna` | `gpt-6-luna` in turn contexts |

The Claude gateway rejected the experimental `context_management` field with
HTTP 400. The passing run used its documented
`CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1` setting. Both credential checks
observed the control variable in the tool subprocess and zero copies of the
provider key. Codex resumed after the test interrupted the registered account's
app-server in its own work directory. [Claude gateway behavior](../crates/claude/PROTOCOL.md)

## Historical checkpoint preview allocations

A draft edit rebuilt a native session pane with 60 transcript rows, 12
turns and three open checkpoint previews. Each preview held four text
patches of 64 KiB each. The control used the same open previews and file
metadata with empty patch text. Each measurement recorded one
`State::detail()` call. Fixture setup, diff loading, the draft edit itself
and browser rendering were outside that call.

| Fixture | Baseline allocations | Baseline bytes | With #96 allocations | With #96 bytes |
| --- | ---: | ---: | ---: | ---: |
| Whole pane with 64 KiB patches | 250 | 815,205 | 178 | 24,705 |
| Whole pane with empty patches | 238 | 28,773 | 178 | 24,705 |
| Difference per text patch | 1 | 65,536 | 0 | 0 |

The baseline was `447f2aa`. The comparison applied the checkpoint preview
implementation only to that baseline. The measurement used
`allocation-counter` 0.8.1 with `nightly-2026-11-15`
(`rustc 1.102.0-nightly (5f4effd57 2026-11-13)`) on x86-64 Linux in Cargo's
test profile.

Use a disposable checkout at `447f2aa` and the
[allocation fixture](verification/preview_allocations.rs) from this tree.
Set `fixed_checkout` to the absolute path of a checkout with the original
#96 fix, before commit snapshots replaced the checkpoint model, then run
from the disposable checkout. These commands reproduce that historical
comparison and are not checks for the current snapshot implementation.

```sh
mkdir -p crates/ui/tests
cp "$fixed_checkout/docs/verification/preview_allocations.rs" crates/ui/tests/
sed -i 's/^    fn detail(/    pub fn detail(/' crates/ui/src/state.rs
cargo +nightly-2026-11-15 add --dev -p ui allocation-counter@=0.8.1
cargo +nightly-2026-11-15 test --locked -j 1 -p ui --test preview_allocations -- --nocapture
```

For the isolated comparison, copy the fixed
`crates/ui/src/checkpoints.rs` and the fixed `Pane::entries` method in
`crates/ui/src/session.rs` into the disposable checkout. Add `Preview` to
the `checkpoints` re-exports in `crates/ui/src/lib.rs` and repeat the test
command.
