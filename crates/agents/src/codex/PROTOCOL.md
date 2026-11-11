# Codex app-server: observed behaviour

These facts come from `s2` runs against codex-cli 0.166.0 with model
`gpt-6-luna` and reasoning effort `low`, on 2026-11-10. Steps refer to the
`s2` checklist. The committed recordings are in `fixtures/codex/`. "Probe"
means a hand-written JSON-RPC session run before `s2` existed; its output is
not committed. Where this file disagrees with the JSON Schema or architecture
§7.2, trust this file and re-check it when the CLI version changes.

## Startup

- `initialize` is answered with `userAgent`, `codexHome`, `platformFamily`
  and `platformOs`. The user agent starts with our `clientInfo.name` and the
  Codex version: `conductor/0.166.0 (Ubuntu 26.4.0; x86_64) WindowsTerminal
  (conductor; 0.1.0)`. We read the version from there. (Every run.)
- The first `initialize` on a cold machine took about 6 s. Later ones took
  0.4 s to 2.4 s. (Probe.)
- If stdin closes before `initialize` is answered, the process exits without
  printing anything. (Probe.)
- `initialized` sent before or after the `initialize` response both worked.
  We send it after. (Probe, every run.)
- A `remoteControl/status/changed` notification follows the handshake on its
  own. (Every run.)
- Every notification carries a top-level `emittedAtMs`. The schema documents
  it on `ServerNotification`. (Every run.)
- `model/list` offered nine models. The only one described as "Fast and
  affordable" was `gpt-6-luna`. (Every run.)

## Threads

- The `thread/start` response arrives before `thread/started`.
  `thread/started` has `params.thread.id` and no `params.threadId`. (Steps 1
  and 4.)
- Each new thread reports the user's MCP servers starting and failing
  (`mcpServer/startupStatus/updated`), tagged with that thread. The user's
  hooks also run (`hook/started`, `hook/completed`). The app-server loads
  `~/.codex/config.toml`. (Steps 1 to 4.)
- Agent messages arrive as `item/started` (empty text), many
  `item/agentMessage/delta`, then `item/completed` with the full text. The
  item has `phase: "commentary"` or `phase: "final_answer"`. (Every turn.)
  The adapter reports each delta as `AgentDelta` and the finished text as
  `AgentMessage`; the deltas join to exactly that text. (`server1.jsonl`.)

## Approvals

- `approvalPolicy: on-request` with `sandbox: workspace-write` wrote `a.txt`
  in the working directory with no approval request. (Probe.)
- `untrusted` with `workspace-write` asks before every command, including
  `pwd && ls -la`, whatever the model thinks of the sandbox. `s2` uses it.
  Steps 1 and 2 passed in 15 of 15 runs, each write asking once. (Probe, and
  steps 1 and 2.)
- `on-request` with `read-only` asks only when the model decides to escalate.
  The model may first run the write in the sandbox, which fails
  (`commandExecution` completes with status `failed`) before the retry asks.
  In 1 of 5 runs it never asked and said the directory was read-only. That is
  why `s2` stopped using this pair. (Step 1.)
- Every approval was `item/commandExecution/requestApproval`. The model wrote
  files with shell commands (`printf ... > a.txt`, `python3 -c ...`), never
  with a patch, so no `item/fileChange/requestApproval` arrived. (Steps 1
  and 2, and the probes.)
- Codex numbers its requests from `0` per process. (Steps 1 and 2.)
- The command approval carries `reason` (such as "May I create a.txt in the
  current directory with the exact requested contents?"), `command`, `cwd`,
  `commandActions`, `proposedExecpolicyAmendment` and, with
  `experimentalApi`, `availableDecisions`. (Steps 1 and 2.)
- `availableDecisions` was `["accept", {"acceptWithExecpolicyAmendment":
  {...}}, "cancel"]`. It omits `decline` and `acceptForSession`. (Steps 1
  and 2.)
- Under `untrusted`, Codex accepted `{"decision": "decline"}` anyway, in 15
  of 15 runs. The command item completed with status `declined`, the model
  said so and the turn completed normally. (Step 2.)
- Codex sends `serverRequest/resolved` with the `requestId` after it reads
  our answer, before the item completes. The actor only reports
  `ApprovalCancelled` when the request is still unanswered at that point. That
  did not happen in any run. (Steps 1 and 2.)

## Interrupt

- `turn/interrupt` needs `threadId` and `turnId`. Codex answers `{}` and then
  sends `turn/completed` with `status: "interrupted"` and `error: null`.
  (Step 3.)
- `turn/interrupt` sent as soon as `turn/started` arrives, before any model
  output, is accepted the same way. The turn completes with
  `status: "interrupted"`, `items: []` and no reasoning or agent message
  item, so `s2` step 3 does not depend on what the model would have said.
  (Step 3, 2026-11-11, five runs.) Earlier runs interrupted after the first
  agent message or item. One of three failed with no output before
  `turn/completed`, so there was nothing to interrupt.
- Before it answers `turn/interrupt`, Codex runs the user's hooks for the
  interrupt (`hook/started` with a run id such as
  `interrupt:4:/home/jim/.codex/hooks.json`, then `hook/completed`). (Step 3,
  2026-11-11.)
- `turn/interrupt` sent on the first `item/agentMessage/delta` is accepted
  the same way, and the turn completes `interrupted`. Between `1` and
  `1` to `9` of the count streamed before it landed, and no
  `item/completed` for the message followed. (Step 3c, 2026-11-11, six
  runs.)
- `turn/interrupt` sent as soon as `turn/start` answers, before
  `turn/started`, is refused with JSON-RPC error -32600 `no active turn to
  interrupt`, although the `turn/start` result already says
  `status: "inProgress"`. The turn then runs to the end and completes
  normally. (Step 3d, 2026-11-11, six runs.) An interrupt for a turn that
  has not started must wait for `turn/started`.
- The thread takes a new turn right after an interrupt. (Steps 3, 3c, 3d,
  `pong`.)

## Several threads on one process

- Two threads with turns in flight at once on one app-server each got their
  own events. Every event named its thread, and `turn/completed` named the
  turn that `turn/start` returned. (Step 4.)
- Notifications from the two threads interleave (`turn/started` for the second
  thread arrived before the first's). (Step 4.)
- Responses can come back out of request order. In the first run the
  `turn/start` response for id 12 arrived before id 11. (Step 4, first run,
  not committed.)

## Resume and exit

- `thread/resume` in a new process returns the same thread id. It sends no
  `thread/started`, but does send `thread/goal/cleared`. The model remembered
  earlier turns and answered `a.txt`. (Step 5.)
- Closing stdin ends the process with exit status 0. (Steps 4 and 6.)
- stderr was empty in the probe. The actor logs it at `debug`.
