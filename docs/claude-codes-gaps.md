# Upstream gaps in `claude-codes` 2.1.323

Conductor drives Claude Code through `claude-codes` (meawoppl/rust-code-agent-sdks).
Spike S1 found the gaps below. Each entry gives what is missing, the evidence, our
local workaround in `crates/agents/src/claude/`, and the fix we propose upstream.
All live evidence is from CLI 2.1.316; the recordings are in `fixtures/claude/`.
No entry needed a `[patch.crates-io]`. The workarounds use the crate's public
types and `ClaudeCliBuilder::build_command()`.

## 1. The clients cannot be driven from an actor

**Missing.** `AsyncClient::receive()` and `RawAsyncClient::next_line()` read with
`read_line` into a buffer local to the future. Used in `tokio::select!` next to a
command channel, a cancelled read drops the part of the line already consumed.
Reads and writes also share `&mut self`, and neither client can split into a
reader half and a writer half. A session that must answer a permission request
while it waits for the next stdout line has no safe way to use either client.

**Evidence.** Source: `client_async.rs` `read_frame_line`,
`client_raw_async.rs` `next_line`. Tokio documents `read_line` as not
cancellation safe.

**Workaround.** We spawn with `ClaudeCliBuilder::build_command()`, own the child's
stdio, read stdout with `Lines::next_line` (cancellation safe), and use
`ClaudeOutput` and `ClaudeInput` only as types.

**Proposed fix.** Add `into_split()` that returns a writer and a line stream, or
keep the partial line in the struct so `next_line` is cancellation safe. Document
which methods are safe in `select!`.

## 2. No `set_permission_mode` control request

**Missing.** `ControlRequestPayload` has `initialize`, `interrupt` and
`apply_flag_settings`, but no `set_permission_mode`.

**Evidence.** Step 5 sent it as raw JSON:
`{"type":"control_request","request_id":"req_2","request":{"subtype":"set_permission_mode","mode":"acceptEdits"}}`.
The CLI answered `{"mode":"acceptEdits"}`, printed `system/status`, and the next
Write ran without a permission request.

**Workaround.** `protocol.rs` builds that line with `serde_json::json!`.

**Proposed fix.** Add `ControlRequestPayload::SetPermissionMode { mode }` and a
`ClaudeInput::set_permission_mode(request_id, mode)` constructor.

## 3. Unknown frame types and control subtypes fail to parse

**Missing.** `ClaudeOutput` has no catch-all variant, and neither has
`ControlRequestPayload`. An unknown `type` or `subtype` is a parse error. For a
`control_request` this is serious: the CLI blocks until it gets a
`control_response`, and the typed path no longer has the `request_id`.

**Evidence.** Source: `claude_output.rs` `Deserialize for ClaudeOutput` ends in
`unknown_variant`. Our tests `unknown_type_becomes_unknown_event_with_the_raw_json`
and `unknown_control_subtype_still_yields_its_request_id` cover the workaround.

**Workaround.** `protocol::parse_line` parses to `serde_json::Value` first. On a
typed parse failure it reads `request_id` from the raw value and replies with an
error `control_response`, as the Python SDK does.

**Proposed fix.** Add `ClaudeOutput::Unknown(Value)` and
`ControlRequestPayload::Unknown(Value)`, keeping the raw JSON.

## 4. `ClaudeCliBuilder` cannot express the flags a session needs

**Missing.**
- No `setting_sources()` method. `CliFlag::SettingSources` exists but the builder
  never emits it.
- `PermissionMode` has no `Auto`, which the SDK lists. It has a `Delegate`
  variant that the SDK does not list.
- `allow_recursion()` (removes `CLAUDECODE`) is only available with the
  `integration-tests` feature.
- `--resume` is emitted as two arguments. The SDK uses `--resume=<id>` so that a
  value starting with `-` cannot be read as a flag.
- `build_command()` does not set `kill_on_drop(true)`.

**Evidence.** `s1` runs with `--setting-sources=project,local` so the user's own
hooks and plugins stay out of the session. The user's settings define a
`PermissionRequest` hook. We did not test what that hook does to a stdio session.
The flag is a precaution, not a reproduced failure.

**Workaround.** `session::command` appends `--permission-mode=<mode>` and
`--setting-sources=<list>` to the built command, removes `CLAUDECODE`, and sets
`kill_on_drop(true)`. The `resumed_session_argv` test pins the resulting argv.

**Proposed fix.** Add `setting_sources()`, `PermissionMode::Auto`, an
always-available way to remove `CLAUDECODE`, the `--resume=<id>` form, and
`kill_on_drop(true)` in `build_command()`.

## 5. `ToolPermissionRequest` drops fields the CLI sends

**Missing.** CLI 2.1.316 sends `display_name` and `description` in every
`can_use_tool`, and the SDK also types `title` and `agent_id`. The struct keeps
none of them, so a UI cannot show the CLI's own description of the request.

**Evidence.** Steps 1, 2 and 3b. For example:
`"display_name":"Write","description":"a.txt"`.

**Workaround.** None needed for S1. Conductor's `PermissionRequested` event will
need these fields in P1.

**Proposed fix.** Add `display_name`, `description`, `title` and `agent_id` as
optional fields.

## 6. `AsyncClient::session_uuid` reads the wrong field

**Missing.** `receive()` stores the first assistant or result message's `uuid` as
the session UUID. That field is the message id, not the session id.

**Evidence.** In the fixtures, every assistant message has a `uuid` different
from its `session_id`. Source: `client_async.rs` `receive`.

**Workaround.** We do not use `AsyncClient`. Our actor reads `session_id` from
`system/init`.

**Proposed fix.** Take the id from `system/init` `session_id`, or from
`session_id` on any message.

## 7. CLI version drift is only a warning

**Missing.** `check_claude_version` runs `claude` from `PATH` instead of the
configured binary. It only logs a warning when the CLI is newer than
`TESTED_VERSION` (2.1.322), and only `with_defaults` and `start_with` call it.
`build_command()` and `AsyncClient::new()` skip it.

**Evidence.** Source: `version.rs`. The `initialize` response already reports the
version: `"claude_code_version":"2.1.316"` (first line of each fixture). That
makes a second process unnecessary.

**Workaround.** None in S1. Architecture §7.1 wants Conductor to refuse
unsupported versions with a clear error. That check belongs in our session spawn,
keyed on the `initialize` response.

**Proposed fix.** Expose `claude_code_version` from a typed `initialize`
response. Offer a check against a supported range that callers can make fatal.

## Differences that need no fix

- `initialize` omits `"hooks": null`. The CLI accepts it (every session).
- User messages go out as a content block array without `session_id`. The SDK
  sends a string with `"session_id": "default"`. The CLI accepts it (every turn).
- The builder always passes `--print`. Multi-turn streaming works with it (all
  steps).
