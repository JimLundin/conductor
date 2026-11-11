# Claude Code stream-json: observed behaviour

These facts come from `s1` runs against Claude Code 2.1.316 with model `haiku`, on
2026-11-10 and 2026-11-11. Steps refer to the `s1` checklist. The committed
recordings in `fixtures/claude/` are one 2026-11-11 run with
`--include-partial-messages`, steps 1 to 6 including 3c and 3d. The 2026-11-10
recordings they replaced are in git history. Where this file disagrees with the
Python SDK source, trust this file and re-check it when the CLI version changes.

## Startup

- The CLI answers `initialize` before it prints anything else. The response
  includes `claude_code_version`, `current_permission_mode`, `commands`,
  `models`, `agents` and `account.tokenSource`. (Every session, first line.)
- `{"subtype":"initialize"}` without `"hooks": null` is accepted. The SDK sends
  `"hooks": null`. (Every session.)
- `system/init` is not a process-start message. The CLI prints it once per turn,
  right after it reads each user message, with the same `session_id` every time.
  (Steps 1 to 3b: five turns, five `init` frames.)
  The adapter reports each one as `TurnStarted`, the CLI's equivalent of
  Codex's `turn/started`.
- With `--setting-sources=project,local`, `init` still lists built-in skills and
  plugins. (Every session.)

## Partial messages

From the 2026-11-11 `s1` runs with `--include-partial-messages`.

- Each API response streams as `stream_event` frames: `message_start`,
  `content_block_start`, `content_block_delta`s, `content_block_stop`,
  `message_delta`, `message_stop`. (Every response.)
- Text arrives as `content_block_delta` with `delta.type: "text_delta"`, in
  chunks of one to thirty characters. The chunks join to exactly the text of
  the assistant message. The adapter reports each as `StreamDelta`. (Steps 1,
  2, 3, 4, 5.)
- Tool input streams as `input_json_delta` chunks. The adapter leaves them
  `Unknown` and reads the input from the assistant message. (Steps 1, 2, 3b, 5.)
- The full assistant message is printed after the last delta but before its
  `content_block_stop` and `message_stop`. (Every response.)
- Every API request is announced by `system/status` with
  `"status": "requesting"`: after each `init`, and after a tool result the
  model goes on to answer. (Every turn.)
- A turn interrupted as it starts prints no stream events. (Step 3.)

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
- Interrupt as soon as `init` arrives, before the model has printed
  anything: the CLI acknowledges it, prints no assistant message and no
  stream events, then the user message `[Request interrupted by user]` and
  `result` with `subtype: "error_during_execution"` and `terminal_reason:
  "aborted_streaming"`. The model's reply never reaches stdout, so `s1` step 3
  does not depend on what the model would have said. (Step 3, 2026-11-11:
  eleven recorded runs, each printing `text=""`.)
- Interrupt during streaming, sent on the first `text_delta`: the CLI
  acknowledges (before any further delta in ten runs, after 44 more in one),
  prints the cut-off assistant message with `"aborted": true` and exactly the
  text streamed so far, then `[Request interrupted by user]`, then
  `content_block_stop`, `message_delta` and `message_stop`, then `result`
  with `subtype: "error_during_execution"`, `is_error: true`,
  `stop_reason: null` and `terminal_reason: "aborted_streaming"`. (Step 3c,
  2026-11-11, eleven recorded runs, all in this order.) Step 3c checks this
  live. It replaces a 2026-11-10 note from one unrecorded run of the old
  step 3, which cut the message off at `"star"`.
- Interrupt sent right after the user message, before `init`: the CLI
  acknowledges it with `{"still_queued": []}` before it prints `init`. Then it
  either aborts the turn as at step 3 (one run of eleven) or ignores the
  interrupt and runs the whole turn to `result/success` (ten runs). The ack
  does not say which. (Step 3d, 2026-11-11.) So an interrupt that reaches
  the CLI before it starts the turn can be lost.
- Interrupt while a permission request is open: the CLI sends
  `control_cancel_request` for that `request_id` before it acknowledges the
  interrupt. It then prints a rejected `tool_result`, the user message
  `[Request interrupted by user for tool use]`, and `result` with
  `subtype: "error_during_execution"`, `is_error: true` and
  `terminal_reason: "aborted_tools"` (13 of 15 runs) or `"aborted_streaming"`
  (2 of 15, `s1-run3.log` and `s1-run4.log`). (Step 3b.)
- `result.subtype` alone cannot tell an interrupt from a failure.
  `terminal_reason` can. (Steps 3, 3b, 3c.)
- The session accepts the next user message normally after an interrupt.
  (Steps 3, 3c, 3d, `pong`.)

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
