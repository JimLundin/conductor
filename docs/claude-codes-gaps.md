# `claude-codes` gaps found in S1, and their fixes

Spike S1 found seven gaps in `vendor/claude-codes` (taken at 2.1.323). Each one is now
fixed in the vendored crate, and the matching workaround in `crates/agents/src/claude/`
is gone. The live evidence is from CLI 2.1.316; the recordings are in `fixtures/claude/`.

## 1. The clients could not run inside an actor

`AsyncClient` and `RawAsyncClient` read with `read_line`, which is not cancellation safe
in `tokio::select!`, and none of the clients could split into reader and writer halves.
Our session actor owns the process and its stdio instead, so nothing used the clients.

**Change.** Deleted `client_async.rs`, `client_raw_async.rs`, `client_sync.rs`, their
examples, `tests/integration_tests.rs`, the live part of `subagent_wrapping_tests.rs`,
`ClaudeCliBuilder::spawn`/`spawn_sync`, the `which` lookup and `CliFlag`. The crate's
features are now `types`, `cli` and `auth`. `build_command()` sets `kill_on_drop(true)`.

## 2. No `set_permission_mode` control request

**Change.** `ControlRequestPayload::SetPermissionMode { mode }` and
`ControlRequestMessage::set_permission_mode` in `src/io/control.rs`. `PermissionMode`
moved there from `cli.rs` so the payload can carry it. `protocol.rs` uses the
constructor instead of `json!`.

## 3. Unknown frame types and control subtypes failed to parse

**Change.** `ClaudeOutput::Unknown(Value)` and `ControlRequestPayload::Unknown(Value)`
keep the raw JSON. A control request whose payload does not parse, unknown or
malformed, keeps its `request_id`. `ControlRequestPayload::subtype()` names the wire
subtype. `audit_frame` and the corpus test count both variants as not wrapped.

`protocol::parse_line` lost its raw-JSON recovery path. The actor still answers every
control request it does not serve with an error `control_response`. The replay test
`unknown_control_request_gets_an_error_reply` checks the bytes it writes.

## 4. `ClaudeCliBuilder` could not express the session's flags

**Change.** In `src/cli.rs`:

- `setting_sources()` and a `SettingSource` enum.
- `PermissionMode::Auto`. `Delegate` is gone; the CLI does not list it.
- `--resume=<id>` as one argument, so an id that starts with `-` is not read as a flag.
- `build_command()` removes `PARENT_SESSION_VARS` (`CLAUDECODE` and the other
  variables a parent Claude Code session sets). `s1` no longer scrubs its own
  environment.

`session::command` is one builder chain. `resumed_session_argv` pins the argv.

## 5. `ToolPermissionRequest` dropped fields the CLI sends

**Change.** `display_name`, `description`, `title` and `agent_id` are optional fields.
`can_use_tool_keeps_every_field_the_cli_sends` round-trips a recorded line. Our
`ClaudeEvent::PermissionRequested` does not carry them yet.

## 6. `AsyncClient::session_uuid` read the wrong field

**Change.** Deleted with `AsyncClient`. Our actor reads `session_id` from `system/init`.

## 7. CLI version drift was only a warning

The old check ran `claude --version` from `PATH`, not the configured binary, and only
logged.

**Change.** `src/version.rs` holds `SUPPORTED_CLI_VERSIONS` (2.1.316 to 2.1.322) and
`check_cli_version`. `InitializeResponse` types `claude_code_version`.
`ClaudeSession::spawn` checks it after the handshake. Outside the range it shuts the
CLI down and returns `Error::UnsupportedVersion { found, supported }`.
`ClaudeOptions::allow_untested_version` runs it anyway with a warning. Widen the range
only after `s1` passes against the new CLI.

## Differences that need no fix

- `initialize` omits `"hooks": null`. The CLI accepts it (every session).
- User messages go out as a content block array without `session_id`. The SDK
  sends a string with `"session_id": "default"`. The CLI accepts it (every turn).
- The builder always passes `--print`. Multi-turn streaming works with it (all
  steps).
