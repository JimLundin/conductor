# Verification boundaries

| Layer | What it checks |
| --- | --- |
| Wire encoding and parsing in `claude`, `codex`, `agents`, `host`, `home` and `link` | Provider JSON lines and host/client messages decode to the expected types. Outbound requests and responses encode the fields the peer expects. |
| Adapter replay in `claude-cli` and `codex-cli` | Recorded CLI exchanges exercise process and thread lifecycle, request correlation, interruption, EOF and resume through the adapter. |
| Provider-event mapping in `claude` and `codex` | Provider messages become the intended common event or request, with the IDs, outcomes and details the host uses. |
| Host tests in `host` and `daemon` | Commands reach the correct session, and receipts and events survive retry and restart. |
| Workspace Git tests | Initial and turn snapshots record commits, unchanged turns can share a commit, and diffs compare consecutive recorded snapshots even when the agent made several commits. Capture failure and restart preserve pending work and branch commits; retained commits stay reachable. Ownership checks, diff limits, original-checkout isolation, finishing and safe removal remain covered. |
| The web app's link and fold in `web/`, over the recorded host feeds | Event streams, reconnects and epoch changes reconstruct session state and the actions offered to a client. |
| Real CLI compatibility | The compiled pins, version checks and recorded exchanges anchor replay to known CLI versions. Re-record provider behavior when changing a pin and document it in the provider's `PROTOCOL.md`. |

Keep assertions at each boundary where they detect a wiring or behavior
failure. The same user action can exercise distinct guarantees in the wire,
and daemon suites.

Snapshot checks cover capture and per-turn comparisons; Continue checks
verify ordinary provider resume after a restart. The dated run below
describes the versions tested then.

## Registered-provider run on WSL2, 21 November 2026

The live-host driver started five sessions for each registered gateway in
isolated provider homes. Each session's creation event identified the expected
account. Both providers passed file approval, interruption on first output,
interruption at send, the subprocess credential check, and crash followed by
Continue with the original codeword. The source checkouts stayed clean.

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
