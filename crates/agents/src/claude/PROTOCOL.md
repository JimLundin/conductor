# Claude Code stream-json: observed behaviour

These facts come from `s1` runs against Claude Code 2.1.316 with model `haiku`, on
2026-11-10. Steps refer to the `s1` checklist. The committed recordings are in
`fixtures/claude/`. Where this file disagrees with the Python SDK source, trust
this file and re-check it when the CLI version changes.

## Startup

- The CLI answers `initialize` before it prints anything else. The response
  includes `claude_code_version`, `current_permission_mode`, `commands`,
  `models`, `agents` and `account.tokenSource`. (Every session, first line.)
- `{"subtype":"initialize"}` without `"hooks": null` is accepted. The SDK sends
  `"hooks": null`. (Every session.)
- `system/init` is not a process-start message. The CLI prints it once per turn,
  right after it reads each user message, with the same `session_id` every time.
  (Steps 1 to 3b: five turns, five `init` frames.)
- With `--setting-sources=project,local`, `init` still lists built-in skills and
  plugins. (Every session.)

## Permission requests

- `can_use_tool` carries `tool_name`, `display_name`, `description` (the file
  name for Write), `input`, `permission_suggestions` and `tool_use_id`. The
  `request_id` is a UUID. No `blocked_path` field was sent. (Steps 1, 2, 3b.)
- `permission_suggestions` for Write in `default` mode was
  `[{"type":"setMode","mode":"acceptEdits","destination":"session"}]`.
  (Steps 1, 2, 3b.)
- A deny with a message comes back to the model as a `tool_result` with
  `is_error: true`. The turn then ends normally with `result/success`. (Step 2.)
- In `default` mode, Bash `echo hi` ran with no permission request. The CLI
  auto-allows commands it classifies as read-only. (Run 3, before step 3b used
  Write instead.)
- After `set_permission_mode` to `acceptEdits`, Write runs with no permission
  request. (Step 5.)

## Interrupt

- The CLI acknowledges `interrupt` with
  `control_response/success` and `{"still_queued": []}`. (Steps 3, 3b.)
- Interrupt during streaming: the CLI prints the user message
  `[Request interrupted by user]`, then `result` with
  `subtype: "error_during_execution"`, `is_error: true`, `stop_reason: null` and
  `terminal_reason: "aborted_streaming"`. (Step 3.) In one run the cut-off
  assistant message also carried `"aborted": true` and the partial text `"star"`.
- Interrupt while a permission request is open: the CLI sends
  `control_cancel_request` for that `request_id` before it acknowledges the
  interrupt. It then prints a rejected `tool_result`, the user message
  `[Request interrupted by user for tool use]`, and `result` with
  `subtype: "error_during_execution"`, `is_error: true` and
  `terminal_reason: "aborted_tools"`. (Step 3b.)
- `result.subtype` alone cannot tell an interrupt from a failure.
  `terminal_reason` can. (Steps 3, 3b.)
- The session accepts the next user message normally after an interrupt.
  (Step 3, `pong`.)

## Resume and exit

- `--resume <session_id>` in a new process keeps the same `session_id` in `init`
  and `result`. The resumed model remembers earlier turns. (Step 4.)
- `set_permission_mode` is acknowledged with `{"mode": "acceptEdits"}` and
  followed by a `system/status` frame. (Step 5.)
- Closing stdin ends the process. It exited 0 when the last turn succeeded and
  1 when the last turn was the interrupted step 3b. That the exit code follows
  the last turn's `is_error` is an inference from two runs, not a tested rule.
- `claude-codes` always passes `--print`. The SDK does not. Streaming input,
  control requests and multi-turn sessions all worked with it. (All steps.)
- stderr was empty in every run.
