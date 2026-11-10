# Gaps in `codex-codes` 0.167.4, fixed in `vendor/codex-codes`

Spike S2 drives `codex app-server` through our vendored copy of `codex-codes`.
We own that copy and fix it in place; nothing goes upstream. Each entry gives
what was missing, the evidence and what we changed. Live evidence is from
codex-cli 0.166.0 (the crate targets 0.167.4). The recordings are in
`fixtures/codex/`.

To compare the vendored types with an installed Codex, export its schema with
`codex app-server generate-json-schema --experimental --out DIR` and run
`python3 -I vendor/codex-codes/scripts/schema_diff.py` on the two bundles. It
prints each property or enum value that exists on one side only.

## 1. The clients cannot serve an actor

**Missing.** `AsyncClient::request()` takes `&mut self` and reads stdout until
its own response arrives, buffering everything else. While it waits, nothing
else can be written: no approval answer, no `turn/interrupt`, no second
request. `respond()` also needs `&mut self`. The only way to send a request is
`request()`, so a caller cannot keep its own pending map. One app-server per
host serves many threads (architecture §8), so requests and approvals for
different threads must interleave. Reads were cancellation safe; the shape was
the problem. `SyncClient` and `RawAsyncClient` have the same shape.

**Evidence.** Source: `client_async.rs` `request`, `respond`, `send_raw`
(private). Step 4 of `s2` had `turn/start` for two threads in flight at once,
and in the first live run the responses came back out of order (id 12 before
id 11).

**Changed.** Deleted `client_async.rs`, `client_sync.rs`,
`client_raw_async.rs`, `stderr_drain.rs`, `version.rs`, the three client
examples and `tests/live_client_tests.rs`. `version.rs` ran `codex --version`
from `PATH`, not the binary we spawn; we read the version from the
`initialize` response's `userAgent` instead. The `sync-client`,
`async-client` and `integration-tests` features are replaced by one `cli`
feature (`AppServerBuilder`, `auth_local`), and the crate no longer depends on
tokio. `AppServerBuilder::build_command()` now returns a
`std::process::Command`; `spawn` and `spawn_sync` are gone. Our actor in
`crates/agents/src/codex/server.rs` owns the child's stdio, as S1 does for
Claude.

## 2. Types are generated from the non-experimental schema

**Missing.** `protocol_generated/types.rs` was generated from the schema
without `--experimental`. A client that sets `experimentalApi` (§7.2 requires
it) receives fields the types drop. For approvals this loses
`availableDecisions`, the native options §7.2 says to preserve, and
`additionalPermissions`.

**Evidence.** `schema_diff.py` against the 0.166.0 `--experimental` bundle
reports `CommandExecutionRequestApprovalParams: prop availableDecisions only
in installed` and the same for `additionalPermissions`. Steps 1 and 2 received
`"availableDecisions": ["accept", {"acceptWithExecpolicyAmendment": ...},
"cancel"]`.

**Changed.** Added `available_decisions:
Option<Vec<CommandExecutionApprovalDecision>>` and `additional_permissions:
Option<Value>` to `CommandExecutionRequestApprovalParams`. Test:
`command_approval_keeps_experimental_available_decisions`. The generator
(`scripts/codegen_protocol.py`) was never vendored, so `types.rs` is now
edited by hand; its header says so.

The diff also lists experimental fields on `ThreadStartParams`,
`TurnStartParams`, `Thread` and others. We send none of them and serde ignores
unknown fields on input, so they are not fixed. Rerun the script when a type we
send or read changes.

## 3. Two `RequestId` types that cannot be compared

**Missing.** `jsonrpc::RequestId` (`String` or `Integer`) is the envelope id.
The generated `protocol::RequestId` (`Variant0(String)` or `Variant1(i64)`) is
a second type, used by `ServerRequestResolvedNotification::request_id`. The
crate root re-exports the first and shadows the second. A caller cannot match
`serverRequest/resolved` against the request it resolves without converting
by hand.

**Evidence.** Our first build failed with ``expected `codex_codes::RequestId`,
found `codex_codes::protocol::RequestId` `` when mapping
`serverRequest/resolved`. Codex sends it after each approval answer (steps 1
and 2).

**Changed.** The generated enum is replaced by
`pub use crate::jsonrpc::RequestId;`. Test:
`server_request_resolved_id_matches_the_envelope_id`.

## Not changed

- **A known method whose params no longer parse is an error.**
  `Notification::from_envelope` and `ServerRequest::from_envelope` return
  `Err` when a modeled method's params do not fit, and `Unknown` only for an
  unknown method. Our boundary (`crates/agents/src/codex/protocol.rs`) turns
  such a notification into `Unknown` with the raw JSON, and such a request
  into an error reply that still carries its id, so the turn does not hang.
  Tests: `known_method_with_drifted_params_becomes_unknown`,
  `unknown_server_request_still_yields_its_id`.
- **`emittedAtMs` on notifications.** Every 0.166.0 notification carries a
  top-level `emittedAtMs` (166 of 180 lines in `server1.jsonl`; the rest are
  responses and requests). `JsonRpcNotification` drops it. We do not use it
  yet.
- **Version skew.** The crate models 0.167.4 and the installed CLI is
  0.166.0. Against the 0.166.0 schema the vendored types have extras that
  0.166.0 never sends (`InitializeCapabilities.requiredHumanApproval`,
  `ItemCompletedNotification.contentTokenCount`), which is harmless.
