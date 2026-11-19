# Conductor architecture

How Conductor is built today, and the designs for what comes next. What is
built and the order of the next slices are in `docs/roadmap.md`. The wire
types are documented in `crates/proto/src/lib.rs`, which is the source of
truth for the protocol; this page explains the shape around them.

## 1. What it is

One app that runs and supervises **Claude Code** and **Codex** sessions on
several of your machines, with no server of ours in the path.

- **Hosts** run `conductord`, which drives the `claude` and `codex` CLIs in
  their native protocols and keeps each host's event log.
- **The client is one web app** (Dioxus compiled to WebAssembly), served as
  static files from GitHub Pages and installed as an app: Chrome, Edge or
  Safari's Add to Dock on the desktop, Add to Home Screen on the iPhone.
  There is no native client and no Apple Developer account.
- **Traffic is end-to-end encrypted.** Clients reach hosts over iroh through
  n0's public relays (§4).

### Goals for v1

- Run Claude Code and Codex sessions on any paired host, one git worktree per
  session.
- Show every host in one interface, on the desktop and on the iPhone.
- Handle approvals, questions, interrupts, resume and per-turn diffs from any
  device.
- Catch up correctly after a disconnect, such as a phone waking up.
- Rust everywhere. The only handwritten JavaScript is the service worker Web
  Push requires.

### Not in v1

A terminal emulator, agents other than Claude Code and Codex, a planner or
orchestrator agent, kanban, CI and review routing, an embedded browser
preview, cloud workers, multi-user use.

## 2. Decisions

| # | Decision | Why |
|---|---|---|
| D1 | Rust for the daemon, the client core and the UI | One set of types end to end. The agents' SDKs are thin wrappers over stdio protocols we speak directly. |
| D2 | One Dioxus web app for every device | Dioxus renders real HTML, which mobile Safari needs for keyboard, scrolling and selection. An installed web app behaves like a native one on the desktop and the iPhone, so there is one build and one test target. A thin native shell can come later if a need appears (tray, global hotkey). |
| D3 | iroh 1.x through n0's public relays; the relay map is one setting | Connections are authenticated and encrypted and dialed by public key. A browser can't send UDP, so in a browser every iroh connection is relayed. |
| D4 | Each host is the only source of truth for its own sessions | No replication between hosts. Clients connect to each host and merge. Catching up is "events after seq N". |
| D5 | Trust comes from a fleet root key that signs device certificates (planned, §6) | Hosts store only the root's public key, so pairing a device once gives it access to every host. |
| D6 | Speak each agent's native protocol, with owned wire types for only the messages we use | Claude's stream-json control protocol and Codex's app-server JSON-RPC carry approvals, interrupts and resume. Lowest-common-denominator protocols (ACP) lose them. |
| D7 | Shell out to the `git` CLI | Worktrees, hooks and the user's git config behave exactly as on the command line. |
| D8 | Length-prefixed postcard frames over bidirectional streams | Compact, and our own Rust types on both ends. postcard isn't self-describing, so every change to a wire type bumps `ALPN`. |
| D9 | Hosts on Linux/WSL, macOS and Windows; development in WSL | The developer works in WSL. |

## 3. The model: one session shape for both agents

The adapters translate provider tool calls, file changes and outcomes into
the shared `proto` types. Each provider's wire crate maps its events into
the session steps of the provider-agnostic `crates/agents`; the daemon
assigns request ids and applies session state and approval wording. The app only ever sees the provider-neutral model,
so one UI and one set of commands work for both agents.

- **Commands:** `StartSession { provider, directory, model, effort }`,
  `SendMessage`, `Interrupt`, `Continue`, `SetEffort { session, effort }`,
  `Resolve { request, Approve | ApproveForSession | Deny }`, `Answer {
  request, answers }`. Each command
  gets one `CommandResult`. A reply that doesn't fit its request (a
  decision for a question, the wrong number of answers, `ApproveForSession`
  where none is offered) is refused as `WrongReply`.
- **Events:** a session is created, becomes ready, closes, and becomes
  ready again after a `Continue`; a user message; the agent's reasoning and
  the assistant's text, each streamed as deltas and then as the finished
  text; tool calls (`Command`, `FileChange`, `Other`) started and finished;
  approval requests opened and resolved (first decision wins, and a late
  one is told who decided); questions asked and answered; a turn
  completes, is interrupted or fails; the session's tasks change
  (`TasksChanged`, the whole list); its effort changes (`EffortChanged`);
  a sub-agent starts, acts and ends (`Subagent`).
- **Model and effort:** `Welcome` lists, for each instance the host can
  run (§5.2), the models its CLI offers with the instance's current
  credential (`Models`: each model's id, name, description and efforts,
  and the default). The host lists them when it starts and again after
  each credential change, for each instance.
  `StartSession` may name one of them and one of its efforts (`low` to
  `ultra`); a model or an effort the host doesn't list is refused as
  `NotOffered`. With none named, the session gets the default: a Codex provider
  credential's model, which is what its gateway serves; else the host's
  `--model <provider>=<model>`; else the one the CLI marks as its
  default. The list comes from the CLI even with a credential. `SessionCreated` carries the model and effort
  the session runs with. A `Continue` after a credential change whose
  models no longer include the session's (neither listed nor the default)
  runs on the new default, keeping the effort if that model takes it, and
  logs `ModelChanged` with the reason first. Otherwise the model is fixed
  for the session; `SetEffort` changes the effort (below).
- **Effort between turns:** `SetEffort { session, effort }` sets the
  effort of the session's later turns. It must be one the session's model
  (or, with none named, the default) lists, else `NotOffered`, by the same
  rule as `StartSession`: a model the host doesn't list takes any effort.
  The same effort again is `Accepted` and logs nothing. The host checks it
  against what the session's instance offers now, so it needs the instance
  available (`InstanceUnavailable`). During undo recovery it is refused as
  `UndoInProgress`, like every order. It is taken while a turn runs and on a closed
  session: `EffortChanged { effort }` is logged at once, and the agent gets
  the effort when the next turn the user sends starts, so a running turn
  keeps the effort it began with. Claude gets it in its running process
  (`apply_flag_settings`) right before that message, Codex as `effort` on
  that `turn/start`. A `Continue` or a host restart starts the CLI at the
  new effort.
- **Tasks:** the host keeps the agent's checklist (Claude's `TodoWrite`,
  Codex's plan) as tasks of its own: `Task { id, text, status, holder }`,
  with a host-assigned ULID `TaskId`, a status of pending, in progress or
  completed, and the session that holds it. Each update from the agent is
  the whole list. The session syncs it into the tasks it holds and logs
  `TasksChanged { tasks }`, the whole list in the agent's order, when
  anything changed. A step keeps the id of the first held task with its
  text that no earlier step took; failing that, if the list kept its
  length, of the held task at its position, unless a step took that one by
  its text; otherwise it is a new task. So a step reworded in place keeps
  its id, but `[a, b, c]` to `[a, c']` makes `c'` a new task rather than
  give it `b`'s id. A task no step keeps is dropped. The log is the store:
  the host restores a session's tasks from its last `TasksChanged`, so ids
  survive `Continue` and a restart. Undoing a turn restores the tasks as
  they were at the checkpoint it goes back to (the last `TasksChanged`
  before it, or none), logging `TasksChanged` with
  `UndoHistoryRestored`. Only the main agent's checklist counts: a Claude
  sub-agent's `TodoWrite` is left out of the session's tasks, and of the
  sub-agent's transcript too.
  Codex's plan `explanation` is not kept, since Claude has none. Handing a
  task to another session is next (`docs/roadmap.md`); `holder` is what it
  will change.
- **Sub-agents:** an agent's tool call may start a sub-agent, a child
  session in the model with no session of its own: its events are its
  parent's, in the parent's log and in seq order. Each is `Subagent {
  agent, event }`, naming it by its provider's id (`SubagentId`, Claude's
  `task_id`): `Started { call, description }` names the tool call that
  started it; `Activity` carries its own transcript, a `Message`,
  `Reasoning` or `ToolCall` (started and finished, each whole, never
  streamed); `Ended { outcome, summary }` ends it as finished, failed or
  stopped, with its report when the provider gives one. The agent API has
  a step for each (`agents::Step::SubagentStarted`, `SubagentActivity`,
  `SubagentEnded`). A sub-agent resumed later starts again under the same
  id. Its tool calls
  ask for approval as the agent's do, so a request names a sub-agent's call
  and gets a card of its own in the session, and a denied call that fails
  is `Declined`. `HostView` keeps each session's sub-agents beside its
  items, as running, ended with its outcome, or stopped when the session
  closed; the app shows each under the tool call that started it, folded to
  its description and status and opening to its transcript. Codex's collab
  agents fit the same model but are not mapped yet (Codex `PROTOCOL.md`).
- **Reasoning:** `ReasoningDelta` streams and `Reasoning` stores the
  finished text, which is the provider's summary of the model's reasoning:
  a few sentences per stretch, never the raw thoughts, so the host stores
  it whole. Each stretch is its own item; the app shows it as a collapsed
  "Thinking" disclosure, whose summary reads "Thinking…" while it streams.
  A stretch without text (Claude through a gateway that omits it) shows
  nothing.
- **Continue:** a session outlives its agent's process. When the process
  exits (a crash, a kill, a stopped Codex app-server), the session closes
  and keeps the id of its conversation. `Continue` starts the CLI again on
  that conversation with the provider's resume (Claude's `--resume <id>` in
  the private home, Codex's `thread/resume` on a new app-server), with the
  same model and effort and the provider's current credential, and logs
  `SessionReady`. The session keeps its id and its log. `Continue` is
  idempotent: on a session whose agent runs it answers `Accepted` and does
  nothing, so a double tap or a second phone ends in the same state. A session closed
  before its first turn starts its CLI afresh, since there is nothing to
  resume yet. Continue also covers host restarts: the host restores sessions
  and their native conversation ids from its durable store before serving
  commands.
- **Requests:** an approval and a question are both requests: one
  `RequestId` space per session, first reply wins, and the agent's
  withdrawal, the end of the turn or the end of the session withdraws one
  still open. `RequestOpened` carries `for_session`, what
  `ApproveForSession` would allow in the agent's words (Claude's rule
  `Bash(git status)`, Codex's command), or nothing if it offers none.
  `QuestionAsked { request, questions }` carries each question's header,
  text, options (label and description) and whether one or many may be
  chosen. Every question also takes typed text, which both agents accept.
  `Answer` carries one answer per question, in order: the chosen labels,
  then any typed text. It resolves as `Answered { answers, by }`.
- **Durable and live:** every event except the streaming deltas (assistant
  and reasoning) is durable:
  it takes the next `HostSeq` and is replayed on catch-up. Deltas go only to
  subscribers that are live, because the finished message replaces them. A
  delta carries the seq of the last stored event, so the client's gap check
  still holds. A client that reconnects mid-reply may miss deltas; the
  finished message repairs the text.

What both providers have belongs in the model. What only one has goes in
typed per-provider options instead of being forced into it: Claude's
permission modes, Codex's sandbox. Raw provider JSON never reaches the UI.

How each agent carries these, from the recordings of the real CLIs:

| | Claude Code 2.1.316 | Codex 0.166 |
|---|---|---|
| Question | `can_use_tool` for `AskUserQuestion`; the questions are in `input` | `item/tool/requestUserInput`, with question ids; needs the `default_mode_request_user_input` feature outside plan mode |
| Answer | `allow` with `updatedInput` = the input plus `answers`, question text to answer, labels joined with ", " | RPC result `{answers: {<id>: {answers: [...]}}}` |
| Withdrawn | `control_cancel_request` | `serverRequest/resolved` after the interrupted turn ends |
| For the session | `allow` with `updatedPermissions`: the suggested allow `addRules`, destination `session`; with none suggested, not offered | `acceptForSession` on a command or a permissions request, taken though `availableDecisions` omits it (a recorded Codex run on 2026-11-12); not offered on a file change, which no run has recorded |
| Refused | Duplicate question texts, by the CLI before it asks | A question with `isSecret`, by the host, which logs and replays every answer |
| Models | The `initialize` response's `models`, with `supportedEffortLevels`; a `default` row | `model/list`, with `supportedReasoningEfforts` and `isDefault` |
| Effort | `--effort`; `get_settings` reports it back | `model_reasoning_effort` in the thread's `config`; `thread/start` reports it back |
| Effort change | `apply_flag_settings` with `{effortLevel}` in the running process; `get_settings` reports it back | `effort` on `turn/start`, for that turn and later ones; `thread/settings/updated` reports it back |
| Plan | A `TodoWrite` call, `input.todos: [{content, status, activeForm}]`, offered with `CLAUDE_CODE_ENABLE_TODO_TOOLS=1` and `CLAUDE_CODE_ENABLE_TASKS=0`; asks no permission | `turn/plan/updated`: `explanation` and `plan: [{step, status}]`, with `tools.update_plan.enabled` in the thread's `config` |
| Reasoning | `thinking_delta`s and a `thinking` block; text only with `--thinking-display summarized` and experimental betas on | `item/reasoning/summaryTextDelta` and the item's `summary`, with `model_reasoning_summary` set |
| Resume | `--resume <session_id>` in a new process | `thread/resume { threadId }` on a new app-server |
| Sub-agent | The `Agent` call; `system/task_started` (`task_type: "local_agent"`, `task_id`, `tool_use_id`, `description`), its frames with `parent_tool_use_id` and `agent_id`, `system/task_notification` (`status` `completed`, `failed` or `stopped`, `summary`) | In the schema only: a `collabAgentToolCall` item (`spawnAgent`, the new thread in `receiverThreadIds`) and `subAgentActivity` items; not recorded, not mapped |

The `AskUserQuestion` tool call is not shown as a tool call; its question
card is. Nor is a `TodoWrite` call; the session's tasks are.

Next in the model (`docs/roadmap.md`): handing tasks between sessions
(#30), and Codex's sub-agents (#31).

## 4. Transport

### 4.1 Streams

ALPN is `conductor/<n>`. Every bi-stream starts with one `Open` frame from the
client:

| Stream | Purpose |
|---|---|
| `Open::Hello` (first) | Authenticates and returns `Welcome { host, epoch, head_seq, instances, catalog }`. `instances` is each instance's availability, by its provider and account; `catalog` is how each provider presents itself and the credential forms it takes (§6.3). |
| `Open::Command` | One `Command` → one `CommandResult`, then the stream closes. |
| `Open::Subscribe { epoch, after_seq }` | Replays the log after the cursor and the sign-ins running, then streams live, each frame a `Feed`: an `Event`, or a live-only `SignIn` step (§6.3). A cursor from another epoch is refused. |

Each frame is a `u32` length and a postcard body.

### 4.2 iroh

- `Endpoint::builder(presets::N0)` with a custom relay map: n0's relays named
  without the root dot, because WebKit refuses the dotted name's certificate
  (`net::without_root_dot`, `net::platform::n0_relays`).
- The host waits for `Endpoint::online()` and prints a ticket with its node id,
  home relay and the auth token (`proto::Ticket`), as a QR code and a
  `#join=` link. The fragment never reaches the static server.
- A dial that stalls for 1.5 s is hedged with a second dial on a fresh
  endpoint. The first to connect wins and the other is retired; when both
  fail, the link keeps the endpoint of the last dial that had one.
  `client::link` decides this per attempt; `net` binds, dials and closes.
- The subsequent Hello exchange has a 10 s deadline covering stream open,
  write and reply. A timeout closes that connection and retries with the
  link's existing jittered backoff, preserving its cursor. Debug logs for
  `client::link` and `daemon::serve` identify each local connection and peer,
  with elapsed times at Hello/Welcome boundaries and the host's registry/log
  waits. They do not record Hello contents or credentials.
- In the browser, iroh needs the `tls-ring` feature, so building the web app
  needs clang for wasm32.
- Tests use `presets::Minimal` with relays disabled, on loopback.

There is no WebRTC upgrade: iroh over n0's relay carries all traffic, and
that is enough for this app's text-sized traffic.

## 5. The host (`conductord`)

### 5.1 Actors

Every piece of state has one owner task; handles talk to it over channels.

- **Log** assigns each durable event its seq, keeps the events, and hands a
  new subscriber its replay and the live broadcast in one turn, so nothing
  falls between them. A subscriber that falls behind the broadcast is dropped
  and resubscribes from its cursor.
- **Registry** maps session ids to session handles and starts sessions,
  and keeps the host's instances. Credential saves update its current
  configuration immediately. Model discovery and Codex reconfiguration run
  in at most one pending future per instance, polled alongside registry
  requests. New saves supersede queued work; old results cannot replace
  current models or failures. Each instance stays unavailable while
  refreshing, and reports a failure if discovery fails. Per-instance
  ordering also prevents older Codex reconfiguration from running after a
  newer one. An added instance stays unavailable until its CLI has
  started, which runs beside registry requests the same way.
- **Session** owns one agent's runtime (§5.2), its turn and its open
  requests. An interrupt that arrives before the agent began the turn is
  held until it does. An open request is withdrawn when the turn ends. It
  outlives the agent's process: once that exits it takes only `Continue`,
  which starts the agent again on the same conversation.
- **Serve** accepts connections, checks `Hello`, dispatches commands and
  feeds subscriptions.

### 5.2 Agents

A session asks one interface for its agent, whichever provider runs it:
the runtime protocol `agents::runtime`. The session sends a `Command`
(start or resume at a history anchor, send, interrupt, reply, list or
revert turns, list models, set credentials, shut down) and waits for its
`Outcome`; meanwhile the agent reports `Step`s. The handle is
`runtime::Runtime`, a channel pair.

The daemon knows each provider only by the `runtime::Plugin` it is handed,
in a `daemon::start::Plugins` map keyed by `Provider`. A plugin is data
plus one function:

- `profile: agents::Profile`, data from the provider's core
  (`claude::profile`, `codex::profile`): the `Provider`; its
  `proto::ProviderInfo` (the label the app shows, a note for the
  new-session form, and the credential forms of §6.3); the pinned build
  (`agents::pin::Cli`); the config its private home starts with; and the
  `--version`, sign-in check and `login` runs as `agents::Invocation`s.
- `settings: runtime::Settings`: the CLI path and the model the command
  line gave it.
- `starter: fn(runtime::Launch) -> runtime::Starter`: the provider's
  runner, started on the CLI that passed its probe, in an instance's
  private home, with the instance's work dir.
- `account: fn(&str) -> Option<String>`: reads the account a signed-in
  home is from what the CLI reports where the profile's `account`
  (`agents::Reported`) says: Claude's `auth status` JSON (`email`), Codex's
  `auth.json` (the ID token's `email`, else `account_id`).

**Instances.** The host doesn't run providers; it runs *instances*, each
one account of one provider, such as Claude through a gateway beside
Claude on a subscription. An instance is keyed by `proto::InstanceId {
provider, account }` on its host (the node, docs/home.md §2): the
provider names the account, not the owner, so the key never changes and
an instance is never renamed. `proto::Account` is `Named(String)` as the
provider names it, or `Own`: the provider's own sign-in before it has
named an account. The instance itself is `proto::Instance { credential,
model, cli }`; the host keeps them as `proto::Roster`, in the credential
store (§6.3). `--cli <provider>=<path>` and `--model <provider>=<model>`
are the defaults of that provider's instances; an instance's own `cli` and
`model` come first, though no command sets them yet.

An instance comes to be when its account is known:

- **A credential** names it in its form: `CredentialForm.account` is the
  id of the field whose value is the account, since neither CLI names one
  for a token or a key (observed with Claude 2.1.316's `auth status` under
  `ANTHROPIC_API_KEY` and `ANTHROPIC_AUTH_TOKEN`, and Codex 0.166's
  `account/read` after an API-key login: `{"type": "apiKey"}`). Claude's
  gateway and Codex's model provider both name their `base_url`.
  `SetCredential { provider, credential }` stores it on the instance of
  that account, adding the instance and starting its CLI if the host has
  none.
- **A sign-in** names it once it ends: the CLI's sign-in check passes and
  the plugin reads the account (`Plugin::account`): Claude's `auth status`
  prints `email` for a `claude.ai` sign-in, and Codex keeps the `ChatGPT`
  sign-in's ID token in `auth.json`. Both are read from the pinned
  binaries' code and Codex's schema, not from a completed sign-in. Every
  provider has its own instance, which is where a sign-in runs: once the
  CLI names the account, the own instance's private home and work dir
  move to the account's instance (`host::registry::signed_in_as`,
  `Change::SignedIn`), whose CLI then starts, and the own instance starts
  afresh for the next sign-in. A sign-in whose account the CLI doesn't
  name leaves the own instance signed in, as before.

So a fresh host has each provider's own instance (`host::registry::defaults`)
and nothing to mint. A CLI given on the command line runs its own instance
signed in however it is; an account's instance without a credential on
such a CLI counts as signed in, since only a sign-in names one.

Each instance gets its own starter, built from its provider's plugin with
its own `Launch`: its private home `<data>/home/<dir>` and its work dir
`<data>/work/<dir>`, where Claude lists its models and Codex's app-server
runs. `<dir>` is `host::registry::dir`: the provider's name for its own
instance, and otherwise `<provider>-` and the first 16 hex digits of the
account's SHA-256, which is safe in a path whatever the account is. So
each instance has its own CLI sign-in and config, its own agents and
credential, and each Codex instance its own app-server (below). Whatever
is per account is keyed by `InstanceId`: the registry's launchers,
availability, models, refreshes and sessions, `Welcome.instances`,
`StartSession { instance }`, and `SessionCreated { instance, provider }`,
so `Continue` and undo run on the session's own instance. What the plugin
describes stays keyed by `Provider`: the catalog's label, note and
credential forms. The app shows an instance by its account, and the own
instance by its provider's label. `SetCredential { provider, credential }`
and `RemoveInstance { instance }` change the instances from the app; each
answers `CommandResult::Instances`, once a credential change has refreshed
the instance and once an added instance's CLI has started.
`RemoveInstance` is refused with `InstanceInUse { sessions }` while a
session runs on the instance (created or ready, and not closed since):
stopping a running agent on someone's behalf would lose its turn. The
instance's closed sessions stay and can't continue, and its home stays,
since their transcripts live in it. A provider's own instance stays.

The starter's agents start with no credential. The host hands them the
instance's credential with `Command::Credential` at start, as it does after
each change, and the `Outcome::Configured` answer names the default model
the credential implies. So each provider's core reads its own credential
and decides what it becomes; the daemon only carries the generic values.
The daemon makes each private home from the profile (§6.2), runs the
probes and installs as `host::start` decides, and sends the plugins'
`ProviderInfo` to clients as `Welcome.catalog`.
`host::session` and the daemon's session actor know no provider, and
`crates/daemon` depends on no provider crate outside its tests.

- `agents`: the provider-agnostic API: the runtime protocol, `Step`,
  `Reply`, the native request and turn ids, `PlanStep`, `Exit`, the
  `Note`s a core logs through its shell, and a provider's `Profile`.
  Pure `no_std`.
- `claude` and `codex`: each provider's wire (stream-json, JSON-RPC), its
  mapping into `agents`, and the state machines that drive it: the process
  actor (`driver`), the runtime that turns commands into calls and events
  into steps (`runtime`), and Codex's `hub`. Each also holds its profile
  and its credential forms, and reads its credential from the generic
  values. Claude's resume fallbacks
  live in `claude::runtime`: a missing conversation with no message seen
  starts fresh, and an undo's fork whose target exists resumes the target.
  Pure `no_std`; they log nothing themselves.
- `runtime`: the shell's handle type, the starter, and `Plugin`.
- `claude-cli` and `codex-cli`: run each CLI's process over tokio and each
  agent's runtime task, with their replay tests, and build their plugins
  (`claude_cli::plugin`, `codex_cli::plugin`). All logging happens here.
- `daemon`: the host behind `conductord`, a library that takes the
  plugins.
- `conductord`: the binary, the one place that names the providers. It
  parses the command line, builds each provider's plugin with its
  settings and calls into `daemon`. Its flags are generic:
  `conductord host [--cli <provider>=<path>]... [--model
  <provider>=<model>]...`, parsed in `host::start`, and `conductord login
  <provider> [--account <account>]`, which picks the instance of that
  account, or without one the provider's own (`host::registry::picked`).
- The Codex hub, `codex_cli::Hub`, is the one task that owns a Codex
  instance's `codex app-server` and routes each thread's events to its
  agent's runtime; its decisions are the state machine `codex::hub`. Each
  Codex instance has its own hub, since the key is fixed in its server's
  environment. It is internal to the Codex runtime: a session never sees
  the shared server.
- `install`: what both CLIs share: the pinned installs, each CLI's home and
  process start and stop.

**Claude Code:** one long-lived process per session, started as

```
claude --input-format stream-json --output-format stream-json --verbose \
       --permission-prompt-tool stdio --include-partial-messages \
       --thinking-display summarized --permission-mode <mode> \
       [--model M] [--effort E] [--resume <session_id> | --session-id <uuid>] \
       --setting-sources user,project,local
```

with `CLAUDE_CONFIG_DIR` set to the host's private home (§6.2), and
`CLAUDE_CODE_ENABLE_TODO_TOOLS=1` with `CLAUDE_CODE_ENABLE_TASKS=0`, without
which 2.1.316 gives a 5.x model no checklist tool.

- stdout is JSON lines: `system`/`init` (once per turn, with the
  `session_id`), `assistant` and `user` messages with content blocks,
  `stream_event` partials and `result`.
- The control protocol runs both ways as `control_request` /
  `control_response`. We send `initialize`, `interrupt`,
  `set_permission_mode` and `apply_flag_settings`; the CLI sends
  `can_use_tool`, answered with
  `allow` (echoing the input as `updatedInput`) or `deny`. Every control
  request we don't serve gets an error reply, or the CLI waits forever.
- An interrupted turn reports `error_during_execution`; only
  `terminal_reason` (`aborted_streaming`, `aborted_tools`) says it was an
  interrupt.
- The `initialize` response's `claude_code_version` is checked against the
  tested range.

**Codex:** one `codex app-server` per Codex instance serves every thread
of that instance.

- Newline-delimited JSON-RPC with no `jsonrpc` field; allow very long lines.
  The handshake is `initialize` with `capabilities: {experimentalApi: true}`.
- We call `thread/start`, `thread/resume`, `turn/start`, `turn/interrupt`
  and `model/list` (when the host starts and after a credential change), and handle
  `turn/started`, `turn/completed`, `item/started`, `item/completed`,
  `item/agentMessage/delta`, `item/reasoning/summaryTextDelta`,
  `turn/plan/updated` and `thread/settings/updated`.
- Approvals arrive as server requests (`item/commandExecution/requestApproval`,
  `item/fileChange/requestApproval`, `item/permissions/requestApproval`,
  `item/tool/requestUserInput`) and `serverRequest/resolved` withdraws one.
  Decisions are `accept`, `acceptForSession`, `decline`, `cancel`; keep
  Codex's own option values, since a label is not always a valid reply.
- Sessions use Paseo's preset: `on-request` approvals with a
  `workspace-write` sandbox.
- Traps (from T3 Code's provider notes): `/goal` can start follow-up turns
  on its own. Their other note, that questions are answered with a new
  user message, does not hold for 0.166's `requestUserInput`, which takes
  an RPC result even when `isBlocking` is `false` (Codex `PROTOCOL.md`).
- Every thread starts with the `default_mode_request_user_input` feature
  on, so the model can ask questions outside plan mode. Codex marks it
  under development and sends a `warning` for it. Every thread also sets
  `model_reasoning_summary = "auto"`: with a custom model provider Codex
  sends no reasoning summary otherwise. And every thread turns the plan tool
  on (`tools.update_plan.enabled`), which 0.166 leaves off.

**Verification:** replay tests feed recorded stdio from `fixtures/` through
the parsers and assert the exact events. The fixtures are recordings of
the real CLIs. No tool in the repo re-records them, so they stay as recorded
until someone re-records them by hand (§6.2).

### 5.3 Durability, worktrees and checkpoints

- **Store (built):** `<data>/conductor.redb`, with typed redb table constants
  and postcard values (storage option C). Its `metadata` records the log
  format, 3 since `SessionCreated` names its instance by its provider and
  account; a database of
  another format doesn't open, and is moved aside to start over. The
  tables are `events`,
  `command_receipts`, `pending_requests`, `conversations` and `metadata`.
  Sessions, their model choices and resolved requests are reconstructed from
  the event log; the native conversation id has its own table. A cursor is
  `(epoch, seq)`; the epoch is created with the database, so wiping the
  database makes clients start over. The ticket token and host key also
  persist, so an existing ticket remains valid after restart. Streaming
  deltas stay live-only.
- **Idempotent commands (built):** receipts are keyed by fleet-unique
  `CommandId`s (128 random bits), so a hedge onto a new transport identity
  still finds the same receipt. One command at a time stages its durable
  events and conversation/request writes, then commits them with its receipt
  before replying. Ordinary commands use one transaction. Undo commits its
  intent and native-history phase separately before their external effects,
  and checkpoint reservations commit before snapshot effects; these explicit
  boundaries broadcast the committed prefix while retaining the command
  lease. A retry with a completed receipt returns its stored result. If
  execution is cancelled or a write fails, the log rolls back only its
  uncommitted suffix and the host stops its transport and provider processes.
  Previously committed undo phases remain pending for recovery. Provider
  stdin writes and the separate credential file are external effects: a
  crash before the database commit can leave a CLI action sent without a
  receipt. Restart withdraws its requests and interrupts its turn; it does
  not promise exactly-once execution inside a CLI.
- **Credential receipts:** the instance commands (`SetCredential` and
  `RemoveInstance`) reserve their command id without holding
  the global event transaction during an instance's refresh or start.
  Other commands and live events can proceed. Duplicate ids wait for the
  original result; superseded saves wait for the newest refresh and return
  its snapshot. The final `Instances` result is committed as the
  receipt before any reply, after an active ordinary transaction finishes.
  The credential file remains outside redb: a crash after saving it but
  before the final receipt can cause a retry to reapply that change. No
  atomic credential-file/receipt guarantee is implied.
- **Restart (built):** the host reconstructs each session closed, withdraws
  requests that were still open and marks in-flight turns interrupted.
  `Continue` resumes the stored native conversation with the current
  credential and the session's model and effort. Before a first message,
  `Continue` starts afresh because Claude has no transcript yet. The daemon
  supervises its log; a failed log stops the host, and provider children are
  killed when their owning tasks are dropped.
- **Planned tables:** `raw_events` (provider wire for remapping), `roster`
  and `push_tokens`, when their features land.
- **The home's state:** the home's remote and the checkouts found, in
  `<data>/home.state` (postcard), outside the log (§5.4).
- **Worktrees and checkpoints (built):** new sessions require a committed
  Git repository and get `<data>/worktrees/<ulid>` on `conductor/<ulid>`,
  initially from committed HEAD. The original working tree and index stay
  untouched. The selected subdirectory is resolved within the managed tree.
  An initial snapshot and each actual finished, failed or interrupted turn
  get immutable `refs/conductor/<ulid>/<checkpoint>` refs. A temporary index
  reads the previous snapshot before `add -A`, `write-tree`, `commit-tree`
  and `update-ref`; branch HEAD never moves. Durable reservations prevent
  checkpoint id reuse after undo or a failed snapshot. A failed snapshot
  retains its native turn boundary and retries before Send or a closed
  session's Continue; an unsuccessful retry rejects that command before
  another turn can change the files. This preserves per-turn diff parents.
- **Removal (built):** `RemoveSession` is refused while a turn runs. It
  stops a running agent (`SessionClosed`), then follows
  `host::workspace::plan`: commit the worktree's untracked and changed
  files onto `conductor/<ulid>` with `commit-tree` and `update-ref` (files
  Git ignores, such as build output, go with the worktree), push
  only that branch to `origin` (never forced; a failed push keeps it
  locally and goes on), `git worktree remove` (or `prune` when the
  directory is gone already), and delete `refs/conductor/<ulid>/`. A
  failed commit or remove stops the removal, logs an `Error` and keeps the
  session. The
  branch stays in the checkout. `SessionRemoved { kept }` ends the log of
  the session, which then refuses every command with `SessionRemoved`.
- **Diff and undo (built):** immutable checkpoint diffs have bounded file
  counts and patch sizes; binary and oversized files get markers. `GetDiff`
  bypasses durable receipts so patches are never copied into the event log
  or receipt table. Undo removes the selected turn and all later turns from
  the client projection and restores the preceding snapshot. Native history
  restoration runs first: Codex uses `thread/revert {beforeTurnId}` and
  verifies the exact retained native turn-id prefix. Claude forks through
  the preceding top-level message UUID with `--resume-session-at` and a
  chosen target id; undoing the first turn starts fresh. Claude creates a
  fork transcript lazily, so its source/through/target recipe stays durable
  until native materialization is confirmed. Restart first tries the chosen
  target and replays the recipe only for the CLI's exact missing-session
  refusal, preserving a target written before its first result was observed.
  An acknowledged message also leaves filesystem snapshot intent before a
  native start notification. If that notification is lost, recovery captures
  the files with an `Unknown` native boundary and disables unsafe historical
  undo; its saved conversation or fork recipe still controls Continue.
- **Undo recovery:** `UndoStarted` commits before provider effects;
  `UndoHistoryRestored` commits before file effects. Files restore with
  `git restore --source=<commit> --staged --worktree -- .` and an
  ownership-verified nonignored clean, preserving ignored files and branch
  HEAD. A recovered file phase also restarts a stopped agent before
  `TurnsUndone` completes recovery. A failed or crashed phase blocks
  Send and Continue, allows immutable diffs and retry of the same target,
  and never silently discards the pending undo. Legacy sessions without
  workspace events continue in their original directory without checkpoints.

### 5.4 The home and its projects (built)

The home (`docs/home.md`) is the owner's git repository of projects,
standards and issues, cloned on every host. Built so far are home steps
3, 4 and 6: the clone, its sync, each project's checkouts, and issues.

- **The home's files.** `projects/<id>/project.toml` lists each project
  under a folder named by its id, a lowercase ULID, which is the
  `ProjectId`. It gives its `name`, unique in the home and what a person
  types to name it, its `title`, which the app shows, its `remote` (absent
  when local only), its `root` (the first commit of its first-parent
  history) and its `profiles`; other keys are ignored, `branch` among
  them: a project's default branch is Git's, and a session's workspace
  branches from its checkout's `HEAD`. A file whose name another project
  has is a problem, as is the other, and neither is listed. Renaming a
  project is an edit of its file, which the next sync picks up; its
  checkouts and sessions follow from its id and its repository, so they
  stay its own. The home is shared by every host, so it never says where a
  host keeps a checkout.
  The `home` core crate parses them (`home::project`), matches checkouts
  (`home::checkout`), says where the clone stands (`home::sync`) and builds
  what the host reports. It is a core of its own rather than part of
  `host`: the home is fleet-wide data in its own files, which issues and
  work will add to, apart from the host's session machinery.
- **The clone.** `conductord host --home <remote>` gives the host the
  home, which it stores in `<data>/home.state`, so later starts need no
  flag. One task, `daemon::home::HomeClone`, owns the clone in
  `<data>/home-repo` (`<data>/home` holds the instances' private homes) and
  runs plain `git`, as the workspaces do. It clones when there is no clone
  and otherwise pulls (`git pull --rebase`, which fast-forwards; a rebase
  that stops is aborted) every five minutes, on `SyncHome`, and before a
  read that needs it (a session started in a project, a clone) unless it
  tried within 30 seconds. A failure is state, not an error:
  `proto::HomeSync` is `Syncing` (first clone), `Synced`, `Behind` (a
  clone, but the last pull failed) or `Failed` (no clone), with
  `Home.synced`, the last successful sync. What the home looks like now is
  on a watch channel, so `Welcome.home` never waits on a pull, and each
  change reaches clients as `Feed::Home`, which replaces the last.
- **Checkouts.** A local repository is a project's checkout when its `HEAD`
  history has the project's root commit and its `origin` is the project's
  remote, compared without scheme, user, `.git` or case; a local-only
  project's checkouts have no `origin`. The host probes, in order, the
  checkouts it found before (host-local, in `<data>/home.state`), then in
  each search root (`--projects <dir>`, repeatable) each directory and each
  directory in those, skipping hidden ones, then its own clones, running `rev-parse --show-toplevel`, `rev-list --max-parents=0
  HEAD` and `remote get-url origin` in each; the decisions are
  `home::checkout`. A project may have several checkouts on a host
  (`proto::Checkout { path, origin: Found | Cloned }`), kept by path in
  `<data>/home.state`. `CloneProject` clones a project with a remote into
  `<data>/projects/<project>` (`Project.cloning` is `Running`, then `Idle`
  or `Failed`). The host only reads the checkouts it finds; sessions run in
  worktrees made from them (§5.3).
- **Sessions in a project.** `StartSession { at: StartAt::Directory |
  StartAt::Project }`: a project starts in its first checkout, or is
  refused with `NoHome`, `UnknownProject` or `NotCheckedOut`. The session
  stores no project of its own: its worktree's repository, which
  `WorkspaceCreated` records, is the checkout, and the app finds the
  project from it (home §2.2).
- **Issues** (home step 6). `projects/<project>/issues/<id>.md`, one file
  per issue in the format of home §3, its name the issue's lowercase ULID
  (`IssueId`); `home::issue` reads it and lists a project's issues in
  `Project.issues`. `CreateIssue { project, fields }` answers
  `IssueCreated { issue }`, with an id the host mints, an `open` status
  and the branch `issue/<id>`. `ChangeIssue { issue, change }` makes one
  small change (`IssueChange`): the title, the body, the labels, the
  priority or the status; an item checked, added or removed, by its text;
  or a note, which the host dates and signs `<host> · <client>`. A change
  rewrites only its own lines of the file, so comments, keys, tables and
  sections a person added stay as written (Windows line ends become
  Unix ones). Both commands keep a receipt (`Ledger::Receipt`), so a
  retry doesn't repeat them, and are refused with `NoHome`,
  `UnknownProject`, `UnknownIssue` or `IssueNotSaved`. A host that dies
  between writing a new issue and committing the receipt opens it again
  on the retry. The host writes the file and commits it in the clone as
  Conductor, one commit per change, keeps the change (`home::issue::Edit`,
  with the value the field had) in `<data>/home-edits.state` and pushes.
  A push the remote refuses, and a sync while changes are kept
  (`home::sync::Git::Replay`), fetch, reset the clone to the remote and
  apply the kept changes again on top (`Mode::Replay`), then push, so the
  host never merges its own commits. A replayed field someone else
  changed since stays theirs unless the change is newer than the file's
  last commit on the remote; a note or an item the file has already
  isn't added twice; a change whose issue is gone is dropped, and one
  whose file no longer reads waits. A push clears only the changes the
  clone holds. Offline, changes stay kept and committed locally,
  `Home.unpushed` counts them, the sync is `Behind` while a push fails,
  and the next sync lands them. A store of them that no longer decodes is
  moved aside to `home-edits.unreadable`.
- **Sessions on issues** (home step 7, started). `StartSession { at:
  StartAt::Issue(issue) }` starts in the issue's project's first
  checkout. Before it does, the host readies the issue's branch,
  `issue/<id>`, there: it fetches the remote's into
  `refs/remotes/origin/issue/<id>` and points the checkout's at it when
  the checkout has none or one the remote's contains, or makes a new one
  at the checkout's `HEAD` (`home::issue::Branch`). The session's own
  branch is cut from it, `OnIssue { issue }` follows `SessionCreated`, and
  the agent's first message is the issue's brief (`home::issue::start`):
  its title, body, checklist and notes, the branch, and what to do when
  done. An `open` or `ready` issue moves to `in-progress`. An instance has
  one session on an issue until it is removed (`host::registry::on_issue`;
  `IssueTaken`); another instance, on this host or another, continues the
  issue from its branch and notes. The app's issue pane lists its
  sessions on every paired host and opens the new-session form on it.

## 6. Planned: the fleet

The goal is one setup running in several places. Every host runs the same
pinned Claude Code and Codex, installed and owned by `conductord`, with the
same settings and the same credentials, except a Codex ChatGPT login,
which each host signs in to once (§6.6). A
host that joins the fleet can run sessions without a manual setup, and a
session can move from one host to another.

D4 still holds. A session lives on one host at a time and its events are in
that host's log. What the fleet shares is configuration, not sessions.

Facts about the CLIs and their terms are marked **verified** (read in the
cited source, or observed on the developer's machine) or **inferred**. They
were checked on 2026-11-12, against Claude Code 2.1.316 to 2.1.322 and
Codex 0.166 to 0.167.

### 6.1 Trust and pairing

Today a ticket's random token stands in for a device certificate. The fleet
replaces it.

- A **fleet root key** (Ed25519) signs `DeviceCert { fleet, device, name,
  role }`. The role is `Owner` for a browser and `Host` for a `conductord`.
  Hosts store only the root's public key and check the certificate in
  `Hello` against it, the authenticated iroh node id and the revocation
  list in the fleet config (§6.5).
- Hosts talk to each other with the same certificates, over iroh, on their
  own ALPN `conductor-fleet/<n>`. Clients never open it.
- The root key lives on one host, the **root host**. It is the first host,
  which creates the fleet with `conductord fleet init`. It signs device
  certificates and every fleet config version, and the other hosts hold no
  signing key. `fleet init` also writes the root key once as a
  passphrase-sealed file for the owner to keep, so a lost root host doesn't
  strand the fleet (F1, §6.10).
- A browser keeps its device key as a non-extractable WebCrypto key in
  IndexedDB, and iOS gives a Home Screen app its own storage, so the phone
  pairs from the installed app. The app shows its public key as a QR code
  and `conductord pair` on the root host signs it. A paired app can approve
  a new one the same way.
- Whoever controls the GitHub Pages deploy controls the code the browser
  runs, and from §6.3 on that code carries credentials in flight. Protect
  the deploy branch, use Subresource Integrity, and keep 2FA on.

### 6.2 The daemon manages its own CLIs

`conductord` installs a pinned version of each CLI into its data dir, checks
it against a recorded hash, and runs only that copy. The user's own `claude`
and `codex`, their versions and their logins are never used or touched.

#### Where the binaries come from

Each CLI has several official channels. Conductor downloads the same
per-platform binary the official installers fetch, straight from the
vendor's release location, so a host needs neither Node.js nor a package
manager.

| | Claude Code | Codex |
|---|---|---|
| Official channels | Native installer (`install.sh`, `install.ps1`, `install.cmd`), npm `@anthropic-ai/claude-code`, Homebrew, WinGet, apt, dnf, apk ([setup], verified) | npm `@openai/codex`, a Homebrew cask, GitHub release binaries ([install.md], [releases], verified) |
| Pinning | The installer takes a version (`bash -s 2.1.89`, [setup], verified), but `install.sh` always downloads the newest build first and has it install the requested one (verified, read in the script). npm pins with `@<version>`. | Release tags `rust-vX.Y.Z` pin exactly; npm pins with `@<version>` ([releases], [npm-codex], verified). The cask can't install an older version (inferred). |
| What Conductor fetches | `https://downloads.claude.ai/claude-code-releases/<version>/<platform>/claude`, the URL `install.sh` uses, with `manifest.json` and `manifest.json.sig` from the same directory (verified, read in the script and fetched) | The release asset `codex-package-<triple>.tar.gz`, on Windows too, from `github.com/openai/codex/releases/tag/rust-v<version>`: the package Codex's own `install.sh` installs, with `bin/codex`, `bin/codex-exec-server`, `bin/codex-code-mode-host`, `codex-resources/bwrap` and `codex-path/rg`. The bare `codex-<triple>.tar.gz` holds only `codex`, and without the rest every command an agent runs fails to start (`install.sh` read, both archives listed, and a scripted run of the real Codex failing 5 of 9 steps on the bare binary, verified) |
| Platforms | `darwin-arm64`, `darwin-x64`, `linux-arm64`, `linux-x64`, their `-musl` variants, `win32-x64`, `win32-arm64` (the 2.1.316 manifest, verified) | `x86_64`/`aarch64` for `unknown-linux-musl`, `apple-darwin` and `pc-windows-msvc` ([releases], verified) |
| Upstream integrity | The manifest lists a SHA-256 per platform and is signed with Anthropic's GPG key, fingerprint `31DD DE24 DDFA B679 F42D 7BD2 BAA9 29FF 1A7E CACE`, for releases from 2.1.89 on. macOS binaries are signed and notarized, Windows binaries are Authenticode-signed, Linux binaries are not individually signed ([setup], verified). | GitHub publishes a SHA-256 digest per asset ([releases], verified). Linux binaries are cosign-signed through Sigstore (`.sigstore` bundles), macOS binaries are codesigned, Windows binaries are signed with Azure Trusted Signing (the release workflows in [codex-src], verified). `codex-package_SHA256SUMS` lists each `codex-package-*` tarball, and each Linux package has a `.tar.gz.sigstore` bundle over the tarball itself. GitHub artifact attestations return 404 for the CLI assets (verified). |
| Updates | Native installs update themselves in the background. `DISABLE_AUTOUPDATER=1` stops the check; `DISABLE_UPDATES=1` also blocks `claude update` and `claude install`, and is meant for distributing Claude Code "through your own channels" ([setup], [env-vars], verified). | The TUI checks GitHub for a newer release and suggests an upgrade command; it doesn't update itself. `check_for_update_on_startup = false` turns the check off. No update check was found in the app-server (`codex-rs/tui/src/updates.rs`, `config_toml.rs` in [codex-src], verified; the app-server part inferred from a search). |

npm was rejected as the channel. It needs Node.js on every host, Codex's
npm package is a launcher plus a 458 MB platform package ([npm-codex],
verified), and Claude Code's package has registry signatures but no
provenance attestation ([npm-claude], verified). The native installer was
rejected because it always runs the newest build before installing the
pinned one.

#### How a pin is verified

A pin is a version plus one SHA-256 per platform. The hashes are recorded
once, by the developer, with upstream's own checks, and every host checks
against the recorded hash.

1. `tools/pin claude <version>` downloads `manifest.json` and its
   signature, verifies the signature against the Anthropic key fingerprint
   above, downloads every platform's binary and checks each against the
   manifest. `tools/pin codex <version>` downloads every platform's package,
   checks each against GitHub's published digest and
   `codex-package_SHA256SUMS`, and verifies the Linux cosign bundles when
   `cosign` is installed. Both write only `crates/agents/src/pin/pins.rs`,
   including the verification result in doc comments. Versions, platform
   hashes and verification provenance stay together in compiled Rust values.
   Updating one provider preserves the other provider's pin.
2. `conductord` compiles the pins in. On start it makes sure the pinned CLI
   is at `<data>/cli/<provider>/<version>/`: `claude` there, and Codex's
   package unpacked whole, with `bin/codex`. It downloads to a temporary
   file, checks the SHA-256 of that file as written to disk, and renames it
   into place (the package is unpacked with the system `tar` beside it
   first, then the directory is renamed), so a crash leaves either nothing
   or a verified install. It holds a lock on `<data>/cli/<provider>/lock`
   throughout, so a second process on the same data dir, such as
   `conductord login` during a first start, waits and then finds the CLI
   installed. A wrong hash, or a download that fails, makes the provider
   `Unavailable` with the reason, as a missing CLI does today. A connection
   gets 15 s and a stalled download 30 s, so a dead network doesn't hold
   up the host's start for long. On Linux it takes Claude's glibc
   build; Codex publishes only musl builds for Linux. A musl host such as
   Alpine is not handled yet.
3. In the same step, under the same lock, it deletes everything else in
   `<data>/cli/<provider>/`: older versions, and any download or unpack a
   crash left. This runs at start, before any session, so no session is
   using an older version.

This keeps OpenPGP and Sigstore out of the daemon. The trust is in the
developer's `tools/pin` run, which upstream's signatures back, and in the
Conductor build that carries the hashes.

`--cli <provider>=<path>` stays, for the fake CLIs in tests and for
development. Without them the daemon runs only the pinned copies.

#### The pin and the fixtures

The pin and the protocol code move together. Bumping a pin is one commit
that:

1. runs `tools/pin` for the new version;
2. widens `SUPPORTED_VERSIONS` and `check_version`
   (`crates/claude/src/protocol.rs`) to include it.

`fixtures/claude` and `fixtures/codex` are recordings of the real CLIs, and
the replay tests run against them. No tool in the repo re-records them, so a
new pin keeps the existing recordings until they are re-recorded by hand.

A test compares the compiled pin with the version recorded in each fixture
and fails when a pinned version has no fixture recorded at that version, or
falls outside the supported range. The rule lives in that test rather than in
this paragraph. So a new pin fails that test until a fixture is re-recorded
by hand at the new version.

#### Bundling is out

Conductor never ships either binary inside its own release. Claude Code's
`LICENSE.md` reads "© Anthropic PBC. All rights reserved. Use is subject to
the Legal Agreements" ([license], verified), which grants no right to
redistribute (inferred, because neither the license nor the terms has a
redistribution clause to rely on). The
legal page also says that running Claude Code in a product requires that
"Claude Code must be installed and run as published by Anthropic" and
that each end user authenticates with their own credentials ([legal],
verified). Conductor follows both. The binary is downloaded unmodified from
Anthropic, and each owner signs in with their own account. Codex is
Apache-2.0 and could be bundled, but one mechanism for both CLIs is simpler,
so Codex is downloaded the same way.

#### Each host gets private CLI homes

| | Directory | Set by |
|---|---|---|
| Claude config, credentials, transcripts | `<data>/home/<dir>` | `CLAUDE_CONFIG_DIR` |
| Codex config, `auth.json`, rollouts | `<data>/home/<dir>` | `CODEX_HOME` |

Each instance (§5.2) has its own home, so each subscription sign-in and
config is separate; `<dir>` is `host::registry::dir` of its key. Each
provider's own instance has its provider's name, so its home is
`<data>/home/claude` or `<data>/home/codex`, the paths below.

`CLAUDE_CONFIG_DIR` moves "all settings, session history, and plugins",
`.claude.json` included, and on macOS keys the Keychain entry to the
directory ([env-vars], [authentication], verified). `CODEX_HOME` holds
`config.toml`, `auth.json` and `sessions/` (`codex-rs` in [codex-src],
verified). So a host's own `~/.claude` and `~/.codex` stay exactly as the
user left them.

Two consequences for how sessions start (§5.2):

- Claude sessions run with `--setting-sources user,project,local` instead of
  `project,local`. The user level is now Conductor's own
  `<data>/home/claude/settings.json`, which sets `cleanupPeriodDays` high
  enough that Claude Code never sweeps a transcript Conductor still needs
  (the default is 30 days, [settings-reference], verified). Conductor deletes
  a transcript when it deletes the session.
- Each Claude process gets `CLAUDE_CODE_PROJECT_DIR_NAME=<session ulid>`, so
  its transcript lives at `<data>/home/claude/projects/<ulid>/` whatever the
  working directory is ([sessions], verified; it needs 2.1.234 or later).
  §6.8 relies on this. Built with the fleet-unique ULID ids and durable
  conversation ids: the host stores the CLI's UUID for `--resume`, while the
  ULID gives each Conductor session its own transcript directory.

`conductord` writes `<data>/home/claude/settings.json` (`cleanupPeriodDays`
36500) and `<data>/home/codex/config.toml` (`check_for_update_on_startup =
false`) on first run only, because the CLIs edit their own config too. Every
`claude` it starts gets `DISABLE_UPDATES=1`. In a private home it also loses
these variables from the host's environment, each of which takes it off the
home's own sign-in: `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` and
`CLAUDE_CODE_OAUTH_TOKEN` take precedence over it,
`CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CODE_USE_VERTEX` and
`CLAUDE_CODE_USE_FOUNDRY` switch to a cloud account, `ANTHROPIC_BASE_URL`
sends requests elsewhere, and `CLAUDE_CONFIG_DIR` names another home.
Codex loses an inherited `CODEX_HOME`. Observed: a shell inside Claude Code
carries `CLAUDE_CODE_OAUTH_TOKEN`, and `claude auth status` then reports a
login in an empty home; with `CLAUDE_CODE_USE_BEDROCK=1` it reports
`"apiProvider": "bedrock"`. With the scrub, each one leaves `"loggedIn":
false`. Even `--version` runs with the
private home set, because `codex --version` writes helper binaries into
`CODEX_HOME/tmp` (observed).

The app signs an instance in (§6.3, *Sign in from the phone*), and
`conductord login <provider> [--account <account>]` does the same on the
host's terminal: both run the CLI's own sign-in against the instance's
private home, then read the account the CLI names, whose instance the
home becomes (§5.2); `login` stores it for the host's next start. Both
run its
provider's `claude auth login --claudeai`, §6.3's per-host mode, which
stores its own credential, or `codex login --device-auth`. A private home
signed in this way counts as configured (`Auth::SignedIn`, §6.3). The host
checks it with `claude auth status` and `codex login status` (15 s at most
each) when it starts and again after a sign-in from the app, so a sign-in
from the app shows at once, while one made on the terminal while the host
runs, or one that expires, shows only after a restart.

Optional host setup on Linux and WSL2: `sudo apt install bubblewrap
socat`. With them Claude runs its commands in its own sandbox and the
host turns on its environment scrub, so Claude's commands can't read a
gateway token (§6.3). Codex sandboxes its commands itself and needs
nothing. Conductor packages neither.

The credential store of §6.3 exists
now, with gateway credentials as its first kind; the shared `setup-token`
token becomes another kind in it.

### 6.3 Credentials, entered in the app

#### What each provider accepts

| | Claude Code | Codex |
|---|---|---|
| API key | `ANTHROPIC_API_KEY`, which takes precedence over a subscription login ([authentication], verified) | `account/login/start { type: "apiKey" }` on the app-server, or `codex login --with-api-key` from stdin ([codex-auth], `v2/account.rs` in [codex-src], verified) |
| Subscription | `claude auth login --claudeai`. On a machine without a browser it prints a URL; the browser that opens it "shows a code instead of returning to `localhost`", and `claude auth login` "reads the pasted code from standard input" ([troubleshoot], verified). | Device code: `account/login/start { type: "chatgptDeviceCode" }` returns `{ loginId, verificationUrl, userCode }` and `account/login/completed` reports the result ([codex-src], verified). It is in beta and the owner must first "Enable device code login in your ChatGPT security settings" ([codex-auth], verified). |
| Long-lived token | `claude setup-token` prints a one-year OAuth token for `CLAUDE_CODE_OAUTH_TOKEN` and saves nothing ([authentication], verified) | None for ChatGPT. `chatgptAuthTokens`, where the host app supplies the tokens, is marked "[UNSTABLE] FOR OPENAI INTERNAL USE ONLY – DO NOT USE" ([codex-src], verified). |

The browser flow, `account/login/start { type: "chatgpt" }`, returns an
`authUrl` whose callback is the app-server's listener on the host's
`localhost:1455` ([codex-app-server], [codex-src], verified). A phone can't
reach that, so it is not offered.

#### Credentials are data

No wire or store type names a provider's credential. Each provider's
plugin describes the kinds it takes in its `ProviderInfo`, which
`Welcome.catalog` carries: a `CredentialForm { kind, label, fields,
account }`, whose `account` names the field that is the account (§5.2), and
each `Field { id, label, kind, placeholder, required }` of kind `Text`,
`Url` or `Secret`. The app draws each form from that description (`ui`
builds it, knowing no provider), and a credential travels and is stored as
`Credential { kind, values }`, its form's values by field id;
`SetCredential` names the provider, and the account field's value the
instance.
The host checks it against the form (`ProviderInfo::check`): the kind is
one the provider takes, every value is a field's, a required field is not
empty, and a `Url` field is plain `http(s)://host/path`. The provider's
core reads its values into its own type (`claude::ClaudeCredential`,
`codex::CodexCredential`) and decides what they become: environment,
config edits and the default model. Another kind is another form in the
provider's core, and the daemon, `host`, `proto`, `client`, `ui` and the
app don't change.

#### Built first: gateway credentials

The owner reaches both models through a gateway today, so that kind is the
first one built. The others in the table become more forms.

- **Claude:** the form `gateway`, with a `Gateway URL` (`base_url`, a URL)
  and a `Token` (`token`, a secret), read into
  `ClaudeCredential::Gateway { base_url, token }`. A Claude
  session gets `ANTHROPIC_BASE_URL` and `ANTHROPIC_AUTH_TOKEN` in its
  environment when it starts. The private home already scrubs both from
  the host's environment (§6.2), so only the stored values reach it.
  Where Claude's sandbox can run, it also gets
  `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1`, so the commands, hooks and MCP
  servers Claude starts don't inherit the token ([env-vars], verified; the
  scrub matches a credential by name or by the shape of its value, from
  2.1.251). On Linux and WSL2 the scrub runs every command in that
  sandbox, which needs `bubblewrap` and `socat`, and without them every
  Bash command fails ([sandboxing], verified; observed). So the host sets
  the scrub on macOS always, on Linux and WSL2 when `bwrap` and `socat`
  are on `PATH`, and never on native Windows, where Claude runs commands
  unsandboxed ([sandboxing], verified). Without it, a stored gateway makes
  the host log "Claude's commands can read the gateway token; install
  bubblewrap and socat to sandbox them" at start and when the gateway is
  set. The base URL reaches commands either way; it is an address, not a
  secret. The live canary in `tools/live-host.sh` has the agent count the
  token in a command's environment: 0 with the sandbox, and 1 without it,
  reported as `leak=unprotected` (observed both). A change applies to the
  next session. Clearing a Claude credential
  doesn't stop the sessions already running on it, unlike Codex below; to
  cut off a leaked token, revoke it at the gateway.
- **Codex:** the form `provider`, with a `Provider name`, a `Base URL`, a
  `Model` and a `Key` (a secret), read into `CodexCredential::Provider {
  name, base_url, key, model }`, whose model is the provider's default.
  Before the app-server starts, the host writes
  `[model_providers.conductor]` (`name`, `base_url`, `env_key =
  "CONDUCTOR_CODEX_KEY"`, `wire_api = "responses"`, the only one Codex
  0.166 knows) and `model_provider` and `model` into the private
  `config.toml`, keeping the rest of the file, and passes the key in
  `CONDUCTOR_CODEX_KEY`. The key is never in the file. Codex 0.166 passes
  variables named like a key to the commands it runs
  (`ignore_default_excludes` defaults to true in
  `config/src/shell_environment_policy.rs` at `rust-v0.166.0`, verified),
  so the config also gets `[shell_environment_policy.filters]
  CONDUCTOR_CODEX_KEY = "exclude"`; the live canary counts it 0 in a
  command's environment (observed). The app-server reloads
  `config.toml` for each `thread/start` (`thread_processor.rs` in
  [codex-src], verified), but the key is in its environment, which is fixed
  when it starts. So a change stops the instance's app-server, its sessions
  close with `CloseReason::Stopped { reason: "new Codex credential" }`,
  and the next thread starts a server with the new key.
- **Protocol:** `SetCredential { provider, credential }` and
  `RemoveInstance { instance }` answer `Instances`, what the host runs
  now, and the client's
  status takes it at once. Another client sees the change at its next
  `Welcome`. Like every command they need a client admitted by the ticket's
  token, which the device certificates of §6.1 replace.
- **Write-only:** `Availability::Available { auth }` reports `Stored {
  kind, url }` (the credential's kind and its `Url` field's value),
  `SignedIn` or `Unmanaged` (a `--cli <provider>=<path>` CLI), and never a
  token or key. A `Credential`'s `Debug` prints only its provider and
  kind, and each provider's own credential type only its kind, so no
  command log line carries a value. A
  provider with no credential, no signed-in private home and no
  development CLI is `Unavailable { reason: "no credential configured" }`.
  A stored credential takes precedence over the home's sign-in. A URL
  with a user, a password, a query or a fragment is refused, since it is
  shown to every client and written into Codex's config.

#### Claude: one subscription token for the fleet, by owner decision

Anthropic's legal page says, verbatim: "developers may not collect, store,
or intermediate Claude.ai credentials or session tokens — sign-in to a
Claude account must complete through Anthropic's own flow." It also says
"Anthropic reserves the right to take measures to enforce these
restrictions and may do so without prior notice" ([legal], verified). The
same page allows provisioning one's own API keys in "a development
environment, secrets manager, or machine image".

**Owner decision F2 (2026-11-12).** Conductor stores one `claude setup-token`
token, encrypted, and copies it to every host through the signed fleet
config, the same way it shares API keys. Storing and copying the token is
what the clause above describes. The owner has read it and accepts that
grey area for their own account and their own tool. The risk is action
against the Claude account, possibly without notice. The binary stays
unmodified, and the sign-in itself still happens on Anthropic's page.

Technically, the token can be shared. `setup-token` mints "a one-year OAuth token",
prints it and saves nothing, and needs a Pro, Max, Ultra, Team or
Enterprise plan ([authentication], verified). It does not refresh: "To
replace an expired token, generate a new one and restart" ([env-vars],
verified). So unlike a `/login` session it has no rotating refresh token,
and many hosts can use it at once without logging each other out
(inferred from those two facts; experiment E2 confirms it). It "can only
make model requests", with no Remote Control or connectors
([authentication], verified), and Conductor needs neither.

Each host passes it as `CLAUDE_CODE_OAUTH_TOKEN` when it spawns `claude`.
`ANTHROPIC_API_KEY` takes precedence over it ([authentication], verified),
so the fleet setting `claude_auth: ApiKey | Subscription` decides which of
the two a host sets, and a host never sets both. The fleet config records
when the token was issued, and the app warns 30 days before the year is
up; renewing is the same sign-in again.

**The fallback stays.** If the owner turns sharing off, or Anthropic acts
on it, each host signs in with Claude Code's own `claude auth login`, which
stores its credential in the host's private `CLAUDE_CONFIG_DIR`, and
Conductor stores nothing for Claude subscriptions. §6.6 describes that mode.

#### Codex: API key shared, ChatGPT login per host

Conductor stores the OpenAI API key in the fleet config. A ChatGPT login
can't be shared, for a technical reason (§6.6). The Codex app-server on
each host makes its own login and keeps it in that host's `CODEX_HOME`.

#### Sign in from the phone

The host runs the CLI's own sign-in and the phone does the browser part.
Built: the per-host subscription sign-in of each provider, on one host.

1. On the credentials screen, each instance's section offers *Sign in*;
   the provider's own instance's signs in an account the host has no
   instance of yet. The app sends `StartSignIn { instance }`, and the host
   runs the
   instance's CLI sign-in in its private home, its stdin, stdout and
   stderr on pipes. A host runs one sign-in per instance at a time and
   refuses a second start (`Rejection::SignInRunning`) until the first ends
   or is cancelled, so two devices never race one CLI; *Cancel*
   (`CancelSignIn { instance }`) kills it, as do a 15-minute limit (Codex's
   one-time code expires then) and the host stopping.
2. **What the CLI prints is read as its plugin describes.** Each provider's
   core gives an `agents::SignIn` in its `Profile`: the run; where the
   output shows the link (`Pick { after, word: Link }`, the first `https://`
   word after that text) and any code to enter on the page (`word: Code`,
   six or more capitals, digits and dashes); whether the page gives a code
   the CLI reads back on stdin (`paste`); and what a failure the CLI prints
   means (`Failure { says, means }`). It exits 0 when it succeeds. The host
   strips terminal colours and keeps the output only until the link is
   there. The daemon, `host`, `proto`, `client`, `ui` and the app read only
   that description.
3. **Claude, per-host.** `claude auth login --claudeai` on pipes prints
   "Opening browser to sign in…", then "If the browser didn't open, visit:
   <url>", whose page "shows a code instead of returning to `localhost`"
   ([troubleshoot], verified), then waits at "Paste code here if prompted >"
   (observed with 2.1.316, `fixtures/claude/sign-in.txt`). The app shows
   *Open the sign-in page* (a new tab) and a password field; the code goes
   in `SubmitSignInCode { instance, code }` and the host writes it to the
   CLI's stdin. A wrong code prints "Login failed: That code wasn't
   accepted…" on stderr and exits 1 (observed). The CLI stores the result
   itself; nothing goes to a fleet config.
4. **Codex, per-host.** `codex login --device-auth` prints the link
   `https://auth.openai.com/codex/device` and a one-time code, coloured even
   on a pipe (observed with 0.166.0, `fixtures/codex/sign-in.txt`), and
   exits once the owner has entered the code on that page. The app shows the
   link and the code with a copy button. Codex writes `auth.json` in the
   instance's `CODEX_HOME`. Its failure texts, a device code login that is
   not enabled and the code timing out, are from the pinned binary's strings
   and not observed. The app-server's `account/login/start { type:
   "chatgptDeviceCode" }` would do the same; the CLI keeps one mechanism for
   both providers.
5. **After it.** A sign-in that exits 0 runs the CLI's sign-in check against
   the home; one that passes makes the instance sign in on its own
   (`OwnSignIn::SignedIn`) and refreshes it, which lists its models. The
   last step, `SignedIn { instances }`, carries what the host runs then, so
   every client's availability turns at once, without a restart or a new
   `Welcome`. A CLI the host was given (`--cli`) is checked the same way
   after a sign-in from the app, though it isn't at start.
6. **Claude, shared token (planned).** The same flow with `claude
   setup-token`, which prints the token instead of storing it; the host
   sends the token to the root host, which puts it in the fleet config
   (§6.5). Relaying the code through the app is an owner decision (F5,
   §6.10). Experiment E1 checks that it works on pipes as `auth login`
   does.
7. **API keys.** The app has a form per kind of credential on each
   instance. `SetCredential { instance, credential }` goes to the root
   host, which puts it in the fleet config, so every host gets it.

**Live only, stored nowhere.** The steps, `Starting`, `Open { url, code,
paste }`, `Finishing`, `SignedIn { instances }`, `Failed { reason }` and
`Cancelled`, reach clients as `Feed::SignIn` frames on the subscription
beside its events. They are not events: the log neither stores nor replays
them, and a new subscription starts with the sign-ins running then, taken
in the same registry turn as the feed of later steps. The sign-in commands
keep no receipt (`Ledger::Unrecorded`), and their answers are `Accepted`
or a rejection, so nothing in the ledger holds a code. `SignInCode`'s
`Debug` prints `SignInCode(..)`, so the command log line shows no code; the
host logs each step's name and a failure's reason, never the link, the
output or a code. The end-to-end test signs in with a wrong and a right
code on the stand-in (`fixtures/fake-login.py`) and finds neither in the
captured log, the stored events and receipts, or any file of the data dir;
the browser run checks the host log, the data dir and the page's storage
and DOM. The Claude token of the shared mode is to go from the host that
minted it to the root host on `conductor-fleet` and never reach the
browser.

`Welcome.instances` says whether the host can run each instance and how it
signs in: `Availability::Available { auth }`, built with `Stored`,
`SignedIn` and `Unmanaged`, where an API key or a subscription token is
one more stored kind, or `Unavailable { reason }`. The sidebar shows a host that needs a sign-in,
with the button.

#### The browser holds a secret only in flight

A `Secret` field, such as the key, is `type="password"` with `autocomplete="new-password"`,
since password managers ignore `off` there. The app sends
the value on the encrypted iroh link, clears the field, and never writes it
to IndexedDB, local storage, the URL or a log. Under the field the app says
so: "Sent to desk-wsl over the encrypted link. This app doesn't keep it;
your browser may offer to save it." A
pasted Claude code gets the same treatment. The app never reads a secret
back; the host reports only which kind is set, with a gateway's base URL.
The credentials screen built first does this for a gateway token and a
Codex provider key.

#### Storage on the host

Conductor stores the fleet's credentials (the API keys and the Claude
token) and its own keys (the host's iroh key and, on the root host, the
fleet root key). They are encrypted in the store with
XChaCha20-Poly1305 under a 32-byte **host key**.

- On macOS the host key lives in the login Keychain, on Windows it is
  sealed with DPAPI for the user, and on Linux it goes in the Secret Service
  when a session bus offers one. Otherwise it is `<data>/store.key`, with
  mode 0600 on Unix. On Windows the file gets the data dir's ACL, which is
  the user's own under the default profile directory and may be wider
  under a `--data-dir` elsewhere. (`<data>/host.key` holds the iroh key
  until the store holds it.) Built so far: the file variant on every
  platform; the platform keystores are follow-up work. A new key is
  written whole and then linked to its name, so two first starts agree on
  one key.
- The credentials are one value, `proto::Roster`, the host's instances
  each with its optional `Credential`, in `<data>/credentials`: a format
  byte (4, since the store holds instances; an older store reads as one
  that doesn't open), a fresh
  24-byte nonce and the postcard sealed with the format and the file's
  purpose as associated data. A save writes and syncs a new file and
  renames it over the old one, then syncs the directory (Unix). It is the
  `instances` field of the fleet config (§6.5), so sharing it is a copy.
- A store that doesn't open (corrupt, or sealed under a lost key) leaves
  the host running with its own instances `Unavailable` for that reason,
  since it may hold a credential that would come before a sign-in.
  `SetCredential` is refused then, so nothing overwrites it. A
  `RemoveInstance` for any of them moves it to
  `<data>/credentials.unreadable` and starts a store of the own
  instances, which is the recovery from the phone. Only a `store.key` that
  can't be read stops the host.
- A Claude gateway token is kept from Claude's commands only where
  Claude's sandbox runs, which on Linux and WSL2 is the optional
  `sudo apt install bubblewrap socat` of §6.2. The Codex key is kept from
  Codex's commands everywhere.
- Most hosts here are WSL or headless Linux, where there is no Secret
  Service. There the encryption protects copies of the store (a backup, a
  store attached to a bug report) and not the running host, because the key
  sits beside it. File permissions are the real protection, as they are for
  the CLIs' own `~/.claude/.credentials.json` and `~/.codex/auth.json`
  ([authentication], [codex-auth], verified). The design says this
  plainly instead of promising more.
- Credentials reach the CLIs only at spawn. Claude gets `ANTHROPIC_API_KEY`
  or `CLAUDE_CODE_OAUTH_TOKEN` in its environment, so Conductor never writes
  either into the CLI's home. The Codex app-server gets `account/login/start { type:
  "apiKey" }` after `initialize`, with `cli_auth_credentials_store =
  "ephemeral"` in its config when the host uses a key, so Codex keeps it in
  memory only (the store options in `config/src/types.rs`, [codex-src],
  verified; that `ephemeral` writes nothing to disk is inferred from its
  name). A host signed in with ChatGPT uses the `file` store, because Codex
  must keep its refreshed tokens across restarts.

### 6.4 Join a host to the fleet

A host joins with a one-time invite, from either side.

- **From the app.** *Add host* asks the root host for an invite and shows
  `conductord join <ticket>`. The ticket holds the root host's node id, its
  home relay and a random 32-byte invite secret that expires after one hour
  and works once. Running the command on the new machine dials the root host
  on `conductor-fleet`, proves the secret, and receives a `Host` certificate
  for its node id and the current fleet config.
- **From the host.** A `conductord` started with no fleet prints its own
  ticket, as it does today. Scanning it in a paired app sends it to the root
  host, which dials the new host and admits it the same way.

Joining is idempotent. An invite already used by the same node id returns the
same certificate, so a join that died after the reply can run again. The new
host then installs the pinned CLIs (§6.2) and can run sessions at once with
the fleet's API keys and Claude token. Codex with a ChatGPT login needs one
sign-in on the new host (§6.6), and the app asks for it as soon as the host
appears.

Leaving is revocation. The root host adds the node id to `revoked` in a new
fleet config version, and every host refuses it from then on. A revoked host
has seen the fleet's credentials, so the app tells the owner to rotate the
API keys, issue a new Claude token, and sign that host out of ChatGPT on
OpenAI's account page. Whether an old `setup-token` token can be revoked
before its year is up is not documented (not verified).

### 6.5 The fleet config: one signed document any host can serve

What the fleet shares is one document:

```
FleetConfig {
    fleet: FleetId,
    version: u64,                 // +1 on every change
    roster: Vec<HostEntry>,       // node id, name, home relay, added_at
    revoked: Vec<NodeId>,         // devices and hosts
    pins: Pins,                   // §6.2
    settings: FleetSettings,      // default models, effort, permission modes
    instances: Roster,            // with their API keys and Claude token; sent only to hosts
}
SignedFleetConfig { config: FleetConfig, signature: [u8; 64] }  // by the root key
```

- **One writer.** Only the root host signs, so versions never fork and there
  is nothing to merge. A change from the app (a new credential, a pin bump, a
  new host) is a command to the root host, which applies it, bumps the
  version, signs and stores it.
- **Any host serves it.** Every host keeps the newest valid config it has
  seen in its store. When two hosts connect on `conductor-fleet`, each sends
  its version; the one behind asks for the document and checks the signature
  and that the version is higher before it adopts it. The root host also
  pushes each new version to every host it can reach.
- **An offline host catches up** the next time it reaches any other host,
  whether that is the root host or not. A host that comes back after a long
  time adopts the newest version, installs any new pin and drops revoked
  devices before it accepts a client.
- **Only hosts get the credentials.** The whole config goes only to peers with a
  `Host` certificate, on the authenticated, encrypted `conductor-fleet`
  link. Each host stores it encrypted under its host key (§6.3).
- **Clients see the rest.** `Welcome` carries the host's config version and its
  roster, so the app learns new hosts from whichever host it reaches first,
  and the sidebar shows a host that runs an older config.

The config is small (a few kilobytes) and changes rarely, so whole-document
transfer is enough. There is no gossip protocol, CRDT or per-field merge.
The single writer costs nothing extra here, because a signature from one key
is already what makes the roster trustworthy.

### 6.6 Which subscriptions a fleet can share

The owner wants a new host to share credentials. API keys and the Claude
`setup-token` token are shared through the fleet config (§6.3). A Codex
ChatGPT login can't be, and Codex falls back to one sign-in per host.

#### Codex: one ChatGPT sign-in per host

This is a technical limit, not a policy choice.

- **Codex rotates its ChatGPT refresh token.** Codex refreshes when the
  access token is within 5 minutes of expiry or the last refresh is more
  than 8 days old, and writes the new tokens back to `auth.json`
  (`TOKEN_REFRESH_INTERVAL`, `persist_tokens` in
  `codex-rs/login/src/auth/manager.rs`, [codex-src], verified). A reused
  refresh token fails with `refresh_token_reused`: "Your access token could
  not be refreshed because your refresh token was already used. Please log
  out and sign in again." (same source, verified). OpenAI's CI guide says
  "Do not share the same file across concurrent jobs or multiple machines",
  because "another machine or concurrent job rotated the token first"
  ([codex-ci], verified). Two hosts on one `auth.json` would log each other
  out.
- **A single refresher isn't available.** It would need Codex to accept
  tokens from Conductor and ask Conductor to refresh them. That is
  `chatgptAuthTokens` with the `account/chatgptAuthTokens/refresh` server
  request, which is marked for OpenAI's internal use only ([codex-src],
  verified). Building on it would break without notice, and Conductor would
  then hold ChatGPT tokens it has no business holding.

So each host signs in to ChatGPT once, from the phone, by §6.3's
device-code flow. After a join, the new host appears with *Sign in to
Codex* when the fleet uses Codex by ChatGPT login. Inside one host, every
Codex thread shares the one app-server, so Codex's own refresh coordination
applies (`auth_storage_commit_guard` in [codex-src], verified).

Whether a ChatGPT sign-in on a second host invalidates the first host's
login is not documented. Experiment E3 tests it. If it does, Codex by
ChatGPT can run on one host at a time, and the app says so before the
second sign-in.

#### Claude: shared by owner decision, with a per-host fallback

Sharing the Claude token is owner decision F2 (§6.10). Anthropic's legal page
says "developers may not collect, store, or intermediate Claude.ai
credentials or session tokens" ([legal], verified), and the owner accepts
that grey area for their own account and their own tool. The risk is that
Anthropic acts against the account, which the same page says it may do
"without prior notice".

So the per-host mode stays designed, for when sharing is turned off. In it,
each host runs `claude auth login` and Claude Code keeps its own credential.
That login is not shareable either. Claude Code refreshes its `/login`
token and coordinates so "only one process refreshes"; before 2.1.211, two
sessions renewing with the same token "revoked the saved login"
([authentication], [troubleshoot], verified). That points to a rotating
refresh token (inferred), so a copied `.credentials.json` would fail the
way a shared Codex `auth.json` does. Inside one host, every Claude process
shares the host's `CLAUDE_CONFIG_DIR`, so Claude Code's coordination
applies. Experiment E2 tests whether a second host's login invalidates the
first.

### 6.7 Fleet-unique session ids and branch names

`SessionId` is a fleet-unique ULID (`crates/proto/src/lib.rs`). A session
that moves keeps its id, and the app merges sessions from every host.

- **Format.** A `SessionId` is a 128-bit ULID: 48 bits of Unix milliseconds
  and 80 random bits, generated by the host that creates the session. The
  wire carries its `u128` value with postcard's variable-length integer
  encoding; as text it is the 26-character Crockford base32 form,
  such as `01JCDZ6Q0Y3K7V8M2N4P5R6S7T`. No coordination between hosts is
  needed, because two ids collide only if they share the millisecond and all 80
  random bits. Ids sort by creation time, which the sidebar uses.
- **Branch.** `conductor/<ulid>`. A session can carry a title, but the title
  is not in the branch name, so renaming never touches git.
- **Worktree.** `<data>/worktrees/<ulid>`.
- **Checkpoints.** `refs/conductor/<ulid>/<turn>`, as in §5.3.
- **Short form.** The UI shows the last 6 characters where space is tight,
  and the full id on hover or long-press.

`HostSeq` and `Epoch` stay per host (D4). An event names its session by id,
so the same id can appear in two hosts' logs after a move (§6.8).

The id change landed with durability, before redb tables keyed on it. It
changes the wire format. The protocol ALPN was then `conductor/10`,
including the appended workspace, checkpoint, diff and undo wire variants.

### 6.8 Move a session to another host

A session is three things: its worktree branch with its checkpoint refs, the
CLI's own transcript, and its events in the host's log. A move copies all
three to the target host and makes the target the session's only owner.
D4 still holds: at any moment one host owns a session, and nothing
replicates continuously.

The app sends `MoveSession { session, to: NodeId }` to the host that owns
the session. That host, the source, drives the move over `conductor-fleet`
under a `MoveId`, and every step is safe to repeat.

1. **Settle.** The source refuses while a turn is running or a request is
   open; the app offers *Interrupt and move*. Otherwise it marks the session
   `Moving`, which refuses `SendMessage`. Then it stops the agent. It ends the
   Claude process, or drops the Codex thread from the Codex hub.
2. **Check the target.** The target must run the same CLI pin, which holds
   in one fleet unless an upgrade is half done, and must be signed in to
   the provider. It must also have the project, meaning a
   clone of the same repository, matched by the root commit and the `origin` URL.
   A target without one clones it from `origin`; a project with no remote is
   sent whole in the bundle.
3. **Send the branch.** The source snapshots the worktree, uncommitted
   changes included, as one more checkpoint (§5.3), then streams
   `git bundle create - conductor/<ulid> refs/conductor/<ulid>/*` excluding
   the commits the target says it already has. The target runs `git bundle
   verify` and `git fetch` from it. A bundle over the iroh stream is the
   default rather than a push, because a push to the user's remote has side
   effects the user didn't ask for (CI runs, a visible branch) and fails
   when the remote is unreachable.
4. **Send the transcript.** The source sends the CLI's session files and the
   target writes them where its own CLI looks, unchanged (§6.8.1).
5. **Send the events.** The source sends the session's durable events. The
   target appends them to its log with its own seqs, after a
   `SessionArrived { from, move_id, source_epoch, source_last_seq }` event,
   and creates the worktree at its own `<data>/worktrees/<ulid>`
   from the last checkpoint.
6. **Commit.** The target registers the session and acknowledges. The
   source then appends `SessionMoved { to, move_id }`, removes its worktree
   and its copy of the transcript, and keeps the branch until the next move
   or deletion. The next `SendMessage` on the target resumes the agent with
   the provider's resume (`--resume <session_id>`, `thread/resume`).

The source owns the session until the target's acknowledgement arrives. If
the source crashes before that, it restarts with the session `Moving`, asks
the target about the `MoveId` and either finishes step 6 or returns the
session to idle. If the target crashed, the source retries from step 2, and
the target's receipt for the `MoveId` makes each step converge.

#### 6.8.1 Resume in another directory

The worktree path differs between hosts, and between operating systems it
can't be made the same. Each CLI handles that differently.

- **Claude.** With `CLAUDE_CODE_PROJECT_DIR_NAME=<ulid>` (§6.2), the
  session's transcript and auto memory are the one directory
  `<data>/home/claude/projects/<ulid>/` ([sessions], verified). The move
  copies that directory as it is. `claude --resume <session-id>` works "from
  any directory" and searches every project for the id, refusing only when
  two copies make it ambiguous ([sessions], verified), and there is one
  copy, so the target resumes it from the new worktree. Each record in the
  `.jsonl` keeps the old absolute `cwd` (verified, observed), and Anthropic
  says the format "is internal to Claude Code and changes between versions"
  ([sessions], verified), so Conductor never rewrites the file. That a
  resume on another machine works is inferred: no document covers it, and
  a request for shared-filesystem sessions was closed as not planned
  ([cc-34024], verified). Experiment E4 tests it.
- **Codex.** Rollouts are
  `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<timestamp>-<uuid>.jsonl`, with an
  index in `session_index.jsonl` and a SQLite state database
  (`codex-rs/rollout`, `codex-rs/state` in [codex-src], verified). Resume by
  id asks SQLite first and falls back to matching file names
  (`find_thread_path_by_id_str`, verified). The move copies the rollout to
  the same relative path in the target's `CODEX_HOME`, and the target calls
  `thread/resume { threadId, cwd: <new worktree> }`. `ThreadResumeParams`
  has the `cwd` override and an unstable `path` field ([codex-src],
  verified); Conductor uses the id and `cwd`, not `path`. That the file-name
  fallback finds the copied rollout and that `cwd` takes effect is
  inferred. Experiment E5 tests both. The rollout records the ChatGPT
  account that created it (`creator_account_id`, verified); a target
  signed in to a different account may hide it from `thread/list`
  (inferred), which Conductor doesn't use.
- **Both.** The model's context still names the old paths. The first
  message after a move gets one line from the host before the user's text:
  "This session moved to another machine. The working directory is now
  `<path>`." Without it, the model may use a path from earlier in the
  conversation that no longer exists (inferred).

#### 6.8.2 What a client sees

- **During.** The session stays in place in the sidebar with *Moving to
  laptop-mac* and the current step. The composer and the interrupt button
  are disabled, and the transcript stays readable.
- **After.** The app is already connected to every host in the roster.
  `SessionMoved` on the source and `SessionArrived` on the target carry the
  same `MoveId`, so `HostView` folds them into one session. It shows the
  target's copy under the target host, keeps the selection on it, and hides
  the source's copy. The transcript reads as one, because the target holds
  the whole history; a divider says where the move happened.
- **If the target is offline** to this client, the app shows the source's
  copy read-only, marked *Moved to laptop-mac (offline)*.
- **If the move fails**, the session returns to idle on the source with the
  reason, and nothing changed on the target that a retry won't reuse.

Diffs and revert for turns before the move still work on the target,
because the checkpoint refs moved with the branch and the events moved with
the log.

### 6.9 Experiments

None of these has been run. Each touches a real Claude or ChatGPT account.

**Owner consent (2026-11-12).** Each experiment may run in the phase that
needs it, in scratch `CLAUDE_CONFIG_DIR` and `CODEX_HOME` directories only,
so the owner's own `~/.claude` and `~/.codex` are never read or written.
The coordinator tells the owner before each run. A throwaway account is
better where one exists.

| | Question | Experiment | Risk to the account |
|---|---|---|---|
| E1 | Does `claude setup-token` print its URL, read the code and print the token when stdin and stdout are pipes? And `claude auth login --claudeai`, for the fallback? | Run each with a scratch `CLAUDE_CONFIG_DIR`, stdin and stdout piped to a script that prints the URL and forwards one line. Complete the sign-in in a browser on another device. Check that `setup-token` printed a token, and run `claude auth status` after `auth login`. | A new token and a new login appear on the account. Sign out of the login afterwards. |
| E2 | Can two hosts use one `setup-token` token at once, and does a second `claude auth login` on another host invalidate the first? | Part 1: set one token as `CLAUDE_CODE_OAUTH_TOKEN` on hosts A and B (scratch dirs, separate machines) and run turns on both at the same time, daily for 4 days. Part 2: sign in A and B with `claude auth login` to the same account, run one turn on each daily for 4 days, past a refresh, and record `claude auth status` on both. | Part 2 may log A out once. |
| E3 | Does a second ChatGPT device-code login invalidate the first, and does sharing one `auth.json` break as documented? | Part 1: device-code sign-in on A and B with the same account. Call `account/read { refreshToken: true }` on A, then on B, then a turn on each; repeat on day 9. Part 2: copy A's `auth.json` to a scratch home C, force a refresh on A, then on C, and expect `refresh_token_reused` on C. | Part 2 is meant to log one copy out. It may also invalidate A's login, which the owner then signs in again. |
| E4 | Does a Claude session resume on another machine with a different working directory? | On A, start a session with a scratch `CLAUDE_CONFIG_DIR` and `CLAUDE_CODE_PROJECT_DIR_NAME`, do two turns that read and edit files. Copy the project directory and the git branch to B under a different path, run `claude --resume <id>` from the new worktree with the stream-json flags, and ask what it did before. Check that it answers from history and edits the new path. | Uses the account's quota; an API key works and avoids the subscription. |
| E5 | Does Codex `thread/resume` find a copied rollout by id and honour `cwd`? | The same as E4 with a scratch `CODEX_HOME`: copy the rollout to the same relative path on B, call `thread/resume { threadId, cwd }`, run a turn, and check the command's working directory. | Uses quota; an API key works. |

E1 decides whether the Claude sign-in needs a pseudo-terminal. E2 confirms
the shared token and settles the fallback. E3 decides whether Codex by
ChatGPT can run on several hosts at once. E4 and E5 decide whether moving
sessions is buildable as designed. Run each before the phase that needs it:
E1 before sign-in from the app, E2 and E3 before fleet and pairing, E4 and
E5 before moving sessions.

### 6.10 Decisions

The owner settled these on 2026-11-12.

| # | Date | Decision |
|---|---|---|
| F1 | 2026-11-12 | **The root key lives on one root host**, the first host, with a passphrase-sealed backup file written at `fleet init`. Fleet changes wait while the root host is offline. Putting the root key on every host was rejected, because then one stolen laptop could admit devices. |
| F2 | 2026-11-12 | **One Claude `setup-token` token is shared across the fleet**, encrypted, through the signed fleet config, against the clause quoted in §6.3, as a grey area the owner accepts for their own account and tool. The risk is action against the account. The per-host `claude auth login` mode stays designed as the fallback (§6.6). |
| F3 | 2026-11-12 | **A Codex ChatGPT login is one sign-in per host**, because refresh tokens rotate and a reused one fails with `refresh_token_reused` (§6.6). |
| F4 | 2026-11-12 | **API keys, Anthropic and OpenAI, are shared fleet-wide** through the fleet config. |
| F5 | 2026-11-12 | **The Claude sign-in code is relayed through the app**, in flight only, to the CLI's stdin on the host (§6.3). |
| F6 | 2026-11-12 | **Codex device-code sign-in is on.** The owner enables it in ChatGPT's security settings. Without it, a ChatGPT login on a headless host needs an SSH tunnel to port 1455, which the phone can't open. |
| F7 | 2026-11-12 | **A move sends the branch as a git bundle over iroh**, not a push to `origin`, so it creates no visible branch or CI run on the remote (§6.8). |
| F8 | 2026-11-12 | **A move between hosts on different pins is refused**, and the app offers to update the target first. The transcript formats are internal and change between versions ([sessions], verified). |
| F9 | 2026-11-12 | **Codex analytics are off** (`analytics.enabled = false` in the hosts' private `config.toml`, [codex-src], verified), as a fleet setting the owner can change. |
| F10 | 2026-11-12 | **Moving sessions comes last** in "Next", after push, because it needs durability, worktrees and the fleet first, and E4 and E5 may change it. |
| F11 | 2026-11-12 | **Experiments E1 to E5 may run** in the phase that needs each, in scratch config directories only, with the owner told before each run (§6.9). |

### 6.11 Sources

[setup]: https://code.claude.com/docs/en/setup
[env-vars]: https://code.claude.com/docs/en/env-vars.md
[sandboxing]: https://code.claude.com/docs/en/sandboxing.md
[authentication]: https://code.claude.com/docs/en/authentication
[troubleshoot]: https://code.claude.com/docs/en/troubleshoot-install.md
[sessions]: https://code.claude.com/docs/en/sessions.md
[settings-reference]: https://code.claude.com/docs/en/settings-reference.md
[legal]: https://code.claude.com/docs/en/legal-and-compliance
[license]: https://raw.githubusercontent.com/anthropics/claude-code/main/LICENSE.md
[npm-claude]: https://registry.npmjs.org/@anthropic-ai/claude-code/2.1.316
[cc-34024]: https://github.com/anthropics/claude-code/issues/34024
[codex-src]: https://github.com/openai/codex/blob/c0cc9fc820f62e4dbdde68214dd6d792d6c8a7ee/
[install.md]: https://github.com/openai/codex/blob/c0cc9fc820f62e4dbdde68214dd6d792d6c8a7ee/docs/install.md
[releases]: https://github.com/openai/codex/releases
[npm-codex]: https://registry.npmjs.org/@openai/codex/0.167.2-linux-x64
[codex-auth]: https://learn.chatgpt.com/docs/auth
[codex-ci]: https://learn.chatgpt.com/docs/auth/ci-cd-auth
[codex-app-server]: https://learn.chatgpt.com/docs/app-server

- Claude Code setup, installers, signing, updates: [setup]
- Claude Code environment variables: [env-vars]
- Claude Code authentication, credential storage, `setup-token`: [authentication]
- Headless sign-in with a pasted code: [troubleshoot]
- Transcripts, resume, `CLAUDE_CODE_PROJECT_DIR_NAME`: [sessions]
- `cleanupPeriodDays`: [settings-reference]
- Terms for running Claude Code and for credentials: [legal]
- Claude Code's license: [license]
- Claude Code on npm: [npm-claude]
- Multi-machine sessions request: [cc-34024]
- Codex source at the commit read (`codex-rs/login`, `codex-rs/app-server-protocol`, `codex-rs/rollout`, `codex-rs/state`, `codex-rs/config`, `.github/workflows`): [codex-src]
- Codex install channels: [install.md]
- Codex release assets and digests: [releases]
- Codex on npm: [npm-codex]
- Codex authentication and device code: [codex-auth]
- Codex in CI, refresh and sharing `auth.json`: [codex-ci]
- Codex app-server: [codex-app-server]

## 7. The client (`crates/client`, `crates/net`)

- `HostLink` (in `net`) is one per host, and the app keeps one live for
  every paired host at once. One task owns each connection and walks
  `Disconnected → Connecting → Authenticating → CatchingUp → Live`,
  reconnecting with jittered backoff, each host on its own. It keeps the
  cursor, so every reconnect resubscribes after the last applied seq, with
  no duplicates and no gaps. A new link starts with no cursor, so the host
  resets what the app holds for it before it catches up.
- `HostView` (in `client`, the pure `no_std` core) folds a host's events
  into what the UI renders: sessions,
  transcripts, tool cards, open requests, what the composer and the
  interrupt button may do. It has no UI types and is unit-tested.
- Both compile to wasm32 for the app. A native build exists only as
  the harness for the daemon's e2e tests and the headless example.

## 8. The app (`crates/app`)

Dioxus web, routed through the URL fragment so GitHub Pages never serves an
unknown path. The app is model, update, view over the pure `no_std` `ui`
crate. `ui::State` holds every paired host, keyed by its endpoint id
(`ui::HostId`), each with its ticket, the name from its last `Welcome`, its
`HostView` and its link `Status` (its instances, catalog and sign-ins).
Beside the hosts it holds what the person has open: the place shown, which
names its host when it is a session, and that place's drafts, forms and
open diffs, dropped when another place shows. `ui::update` turns each
`Intent` (what a person does, a host's link update, a command's answer)
into the next state and `Effect`s as data: send a command to a host, save
the hosts, move the focus. A link update, a credential change and each
`Effect::Send` name their host, and an answer comes back to the host and
the place that asked. `ui::view` describes the whole `Screen`, down to each
string, class and enabled flag, and which pane a phone shows. The Dioxus
components only draw that description and turn events into intents; one
signal in the app shell holds the state and performs the effects, and the
shell keeps a `HostLink` for each host the screen lists, keyed by its
ticket.

- **Several hosts.** The session list has a group per host, headed by its
  name and link state, with its own error line; the detail pane shows the
  selected session of its host. The credentials screen has a section per
  host with its instances and *Forget this host*. A new session picks a host
  first, by default the host of the session shown last, else the first
  live one, then one of that host's instances. Where the app speaks of a
  host it names it.
- **Pairing.** A join link (`#join=`) or a ticket pasted under *Add host*
  adds a host; pairing a host again gives it the new ticket and keeps its
  sessions. Forgetting the last host returns to the pairing screen. A Home
  Screen app on iOS has its own storage and can't receive a join link from
  the camera, so it pairs from its own form.
- **Projects** (§5.4). *Projects* in the band shows each host's home, how
  it syncs and a *Sync* button, then each project the homes list, with its
  checkouts on each host that lists it and *Clone here* where it has none
  and a remote. The status's `home` comes from `Welcome` and `Feed::Home`.
  The new-session form offers the chosen host's projects, those with no
  checkout there disabled, or a directory; a picked project hides the
  directory. A session whose worktree's repository is a project's checkout
  is listed by the project's name.
- **Storage.** Local storage keeps `conductor.hosts`, a JSON object keyed
  by each host's endpoint id in hex, holding its ticket text and name
  (`ui::encode`, `ui::decode`). The first launch after the single-host app
  takes its `conductor.ticket` into the list and removes it.

We port T3 Code's designs, not its code (`apps/web/src/components/`):

| Ours | T3 Code reference | v1 |
|---|---|---|
| Sidebar: hosts, projects, sessions with status | `AppSidebarLayout.tsx`, `ConnectionStatusDot.tsx` | yes |
| Transcript, markdown, code | `ChatView.tsx`, `ChatMarkdown.tsx` | yes |
| Tool-call cards | `CommandBlock.tsx` | yes |
| Approval and question panel | `ComposerContextPermissions.tsx` | yes |
| Diff panel per turn | `DiffPanel.tsx` | yes |
| Branch toolbar, command palette | `BranchToolbar*.tsx`, `CommandPalette*.tsx` | later |
| Terminal | `ThreadTerminalDrawer.tsx` | no |

The phone layout follows T3 Code's `docs/internals/mobile-layout.md`: the
sidebar becomes a stack and approvals get one-tap actions.

**Push (planned):** iOS supports Web Push for Home Screen apps. Each host
keeps a VAPID key, stores each device's subscription, and sends a
content-free push ("desk-wsl: approval needed") when a request opens. The
service worker that shows it is the one handwritten JavaScript file. iOS
suspends a backgrounded web app, which drops its connections; on return the
client catches up from its cursors.

## 9. Testing

- **Replay tests** (`crates/claude-cli/tests`, `crates/codex-cli/tests`):
  recorded CLI sessions through the
  parsers, asserting exact events.
- **Daemon e2e** (`crates/daemon/tests/e2e.rs`): the real daemon with fake
  CLIs and native `HostLink`s on loopback: approvals, interrupts, reconnects,
  epochs, two Codex sessions on one server.
- **Browser e2e** (`tools/e2e-browser.sh`): the web app in Chromium and
  WebKit against the real daemon and the fake CLIs, with screenshots in
  `docs/p1/screens/`.
- **Manual smoke** (`docs/p1/smoke.md`): real CLIs, the desktop browser and
  the iPhone on mobile data.

## 10. References

| Source | License | Use |
|---|---|---|
| pingdotgg/t3code | MIT | Adapter behaviour, receipts, checkpoints, UI designs |
| anthropics/claude-agent-sdk-python | MIT | Claude's control protocol |
| openai/codex (`codex-rs/app-server`, its JSON Schema) | Apache-2.0 | Codex's protocol |
| getpaseo/paseo | Apache-2.0 | Provider option sets, timeline epochs |
| BloopAI/vibe-kanban (`executors`) | Apache-2.0 | Second reference for the clients |
| hardbeat920/monocode | MIT | SSH host bootstrap, provider lifecycle |
| n0-computer/iroh, DioxusLabs/dioxus | Apache-2.0/MIT | Transport, UI |
