# Conductor architecture

How Conductor is built today, and the designs for what comes next. What is
built and the order of the next slices are in `docs/roadmap.md`. The
client link's frames are documented in `crates/link/src/lib.rs`, which is
the source of truth for the link, and the domains' types beside the code
that owns them; this page explains the shape around them.

## 1. What it is

One app that runs and supervises **Claude Code** and **Codex** sessions on
several of your machines, with no server of ours in the path.

- **Hosts** run `orchestrator`, which drives the `claude` and `codex` CLIs in
  their native protocols and keeps each host's event log.
- **The client is one web app** (TypeScript, ADR 0001), served as
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
- Rust on the hosts. The browser app is TypeScript over a wasm iroh pipe
  (ADR 0001).

### Not in v1

A terminal emulator, agents other than Claude Code and Codex, a planner or
orchestrator agent, kanban, CI and review routing, an embedded browser
preview, cloud workers, multi-user use.

## 2. Decisions

| # | Decision | Why |
|---|---|---|
| D1 | Rust for the daemon, the client core and the UI | One set of types end to end. The agents' SDKs are thin wrappers over stdio protocols we speak directly. |
| D2 | One web app for every device (Dioxus at first; TypeScript since ADR 0001) | Dioxus renders real HTML, which mobile Safari needs for keyboard, scrolling and selection. An installed web app behaves like a native one on the desktop and the iPhone, so there is one build and one test target. A thin native shell can come later if a need appears (tray, global hotkey). |
| D3 | iroh 1.x through n0's public relays; the relay map is one setting | Connections are authenticated and encrypted and dialed by public key. A browser can't send UDP, so in a browser every iroh connection is relayed. |
| D4 | Each host is the only source of truth for its own sessions | No replication between hosts. Clients connect to each host and merge. Catching up is "events after seq N". |
| D5 | Trust comes from a fleet root key that signs device certificates (§6) | Hosts store only the root's public key, so pairing a device once gives it access to every host. |
| D6 | Speak each agent's native protocol, with owned wire types for only the messages we use | Claude's stream-json control protocol and Codex's app-server JSON-RPC carry approvals, interrupts and resume. Lowest-common-denominator protocols (ACP) lose them. |
| D7 | Shell out to the `git` CLI | Worktrees, hooks and the user's git config behave exactly as on the command line. |
| D8 | Length-prefixed frames over bidirectional streams: JSON between a client and a host, postcard between hosts | The browser reads JSON with schemas generated from `link`'s root types (ADR 0002, ADR 0003); postcard stays where only Rust reads it. Every change to a type the client link carries bumps `link::ALPN`. |
| D9 | Hosts on Linux/WSL, macOS and Windows; development in WSL | The developer works in WSL. |

## 3. The model: one session shape for both agents

The adapters translate provider tool calls, file changes and outcomes into
the provider-neutral `agents` types. Each provider's wire crate maps its events into
the session steps of the provider-agnostic `crates/agents`; the daemon
assigns request ids and applies session state and approval wording. The app only ever sees the provider-neutral model,
so one UI and one set of commands work for both agents.

- **Commands:** each domain has its own commands, results and refusals,
  and the client link wraps them by domain (`Command::Session`,
  `Instance`, `SignIn`, `Home`, `Push`, `Fleet`; ADR 0003), so
  `Session::Start` below is `Command::Session(SessionCommand::Start)`. A
  session's are `Session::Start { instance, at, model, effort }`,
  `SendMessage`, `Interrupt`, `Continue`, `SetEffort { session, effort }`,
  `Resolve { request, Approve | ApproveForSession | Deny }`, `Answer {
  request, answers }`. Each command
  gets one `CommandResult`, in its domain. A reply that doesn't fit its request (a
  decision for a question, the wrong number of answers, `ApproveForSession`
  where none is offered) is refused as `WrongReply`.
- **Events:** a session is created, becomes ready, closes, and becomes
  ready again after a `Continue`; a user message; the agent's reasoning and
  the assistant's text, each streamed as deltas and then as the finished
  text; tool calls (`Command`, `FileChange`, `Other`) started and finished;
  approval requests opened and resolved (first decision wins, and a late
  one is told who decided); questions asked and answered; a turn
  completes, is interrupted or fails; the session's checklist changes
  (`ChecklistChanged`, the whole list); its effort changes (`EffortChanged`);
  a sub-agent starts, acts and ends (`Subagent`).
- **Model and effort:** `Welcome` lists, for each instance the host can
  run (§5.2), the models its CLI offers with the instance's current
  credential (`Models`: each model's id, name, description and efforts,
  and the default). The host lists them when it starts and again after
  each credential change, for each instance.
  `Session::Start` may name one of them and one of its efforts (`low` to
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
  rule as `Session::Start`: a model the host doesn't list takes any effort.
  The same effort again is `Accepted` and logs nothing. The host checks it
  against what the session's instance offers now, so it needs the instance
  available (`InstanceUnavailable`). It is taken while a turn runs and on a closed
  session: `EffortChanged { effort }` is logged at once, and the agent gets
  the effort when the next turn the user sends starts, so a running turn
  keeps the effort it began with. Claude gets it in its running process
  (`apply_flag_settings`) right before that message, Codex as `effort` on
  that `turn/start`. A `Continue` or a host restart starts the CLI at the
  new effort.
- **Checklists:** each agent reports its latest ordered list of text and
  status (pending, in progress or completed). Both providers use
  `agents::ChecklistEntry`; the host records the whole changed snapshot as
  `ChecklistChanged { checklist }`. Entries have no persistent identity or
  owner beyond their session or sub-agent. Reordering, renaming, duplicate
  text and removal are represented exactly as the provider reports them.
  Repeated identical snapshots produce no event. Main and child
  checklists remain separate; child updates use
  `SubagentEvent::ChecklistChanged`. Replay and Continue retain the last
  snapshot.
  Checklists are displayed in the app and generate no issue notes.
  Codex's plan `explanation` is not kept, since Claude has none.
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
  is `Declined`. The app's fold keeps each session's sub-agents beside its
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
| Instructions | `--append-system-prompt=<text>` at each launch (`--help` of the pinned CLI); it reads `CLAUDE.md` itself | `developerInstructions` on `thread/start` and `thread/resume` (the pinned app-server's schema); it reads `AGENTS.md` itself |
| Plan | A `TodoWrite` call, `input.todos: [{content, status, activeForm}]`, offered with `CLAUDE_CODE_ENABLE_TODO_TOOLS=1` and `CLAUDE_CODE_ENABLE_TASKS=0`; asks no permission | `turn/plan/updated`: `explanation` and `plan: [{step, status}]`, with `tools.update_plan.enabled` in the thread's `config` |
| Reasoning | `thinking_delta`s and a `thinking` block; text only with `--thinking-display summarized` and experimental betas on | `item/reasoning/summaryTextDelta` and the item's `summary`, with `model_reasoning_summary` set |
| Resume | `--resume <session_id>` in a new process | `thread/resume { threadId }` on a new app-server |
| Sub-agent | The `Agent` call; `system/task_started` (`task_type: "local_agent"`, `task_id`, `tool_use_id`, `description`), its frames with `parent_tool_use_id` and `agent_id`, `system/task_notification` (`status` `completed`, `failed` or `stopped`, `summary`) | In the schema only: a `collabAgentToolCall` item (`spawnAgent`, the new thread in `receiverThreadIds`) and `subAgentActivity` items; not recorded, not mapped |

The `AskUserQuestion` tool call is not shown as a tool call; its question
card is. Nor is a `TodoWrite` call; the session's checklist is.

Next in the model (`docs/roadmap.md`): Codex's sub-agents (#31).

## 4. Transport

### 4.1 Streams

ALPN is `conductor/<n>`. Every bi-stream starts with one `Open` frame from the
client:

| Stream | Purpose |
|---|---|
| `Open::Hello` (first) | Authenticates and returns `Welcome { host, epoch, head_seq, instances, catalog }`. `instances` is each instance's availability, by its provider and account; `catalog` is how each provider presents itself and the credential forms it takes (§6.3). |
| `Open::Command` | One `Command` → one `CommandResult`, then the stream closes. |
| `Open::Subscribe { epoch, after_seq }` | Replays the log after the cursor and the sign-ins running, then streams live, each frame a `Feed`: an `Event`, a live-only `SignIn` step (§6.3), the current `Home`, or the current `Instances` offer. A cursor from another epoch is refused. |

Each frame is a big-endian `u32` length and a JSON body written by serde.
A 128-bit id (`SessionId`, `EditId`) is its 26-character
ULID text as the `ulid` crate writes it, in upper case (it reads either
case), a `CommandId` its UUID text as the `uuid` crate writes it, an `IssueId` or `ProjectId` its short text (`home`), and an
`Epoch` its decimal digits, since JavaScript's numbers are exact only to
2^53; `Instances` and `Roster` are lists of `[InstanceId, value]` pairs,
since a JSON key is only a string; bytes are arrays of integers. Stores and host-to-host frames stay postcard.

### 4.2 iroh

- A host binds with a custom relay map: n0's relays named without the root
  dot, because WebKit refuses the dotted name's certificate
  (`net::without_root_dot`, `net::n0_relays`).
- The host waits for `Endpoint::online()` and prints a ticket with its node id,
  home relay and the auth token (`link::Ticket`), as a QR code and a
  `#join=` link. The fragment never reaches the static server.
- The browser dials through `crates/web-iroh` (§7.1): a fresh endpoint per
  dial whose only relay is the ticket's home relay.
- A dial that stalls for 1.5 s is hedged with a second dial on a fresh
  endpoint. The first to connect wins and every other is closed once it
  settles (`web/src/link/dial.ts`).
- The subsequent Hello exchange has a 10 s deadline. A refusal or a timeout
  closes that connection and retries with the link's jittered backoff,
  keeping its cursor. Debug logs for `daemon::serve` identify each
  connection and peer, with elapsed times at Hello/Welcome boundaries and
  the host's registry/log waits. They do not record Hello contents or
  credentials.
- In the browser, iroh needs the `tls-ring` feature, so building the web app
  needs clang for wasm32.
- Tests use `presets::Minimal` with relays disabled, on loopback.

There is no WebRTC upgrade: iroh over n0's relay carries all traffic, and
that is enough for this app's text-sized traffic.

## 5. The host (`orchestrator`)

### 5.1 Actors

`daemon::start` takes a bound iroh endpoint, the provider plugins and the
host's options, including its data directory. It opens the credential store
and durable log, starts the home and registry tasks, and serves the protocol
on the endpoint. It returns a listening `Hosted` handle with the connection
target and a shutdown operation. Callers that need settled offers can await
`Hosted::initialized`. `daemon::serve` binds the production
endpoint with the host's persistent key, calls `start`, prints the ticket
and QR code, and shuts the host down on Ctrl-C.

Every piece of state has one owner task. Handles talk to it over channels.

- **Log** assigns each durable event its seq, keeps the events, and hands a
  new subscriber its replay and the live broadcast in one turn, so nothing
  falls between them. A subscriber that falls behind the broadcast is dropped
  and resubscribes from its cursor.
- **Registry** maps session ids to session handles and starts sessions,
  and keeps the host's instances. The registry core decides session starts.
  The session command lease keeps starts in order through the commit of the
  opening events. Issue branch readiness runs alongside other registry
  requests. The registry checks the directory and instance after the branch
  is ready and snapshots the workspace before recording any opening events.
  The session actor starts the agent after the records are written. The
  issue brief runs separately. The start command waits for the issue's
  progress update before answering.
  Credential saves update the current configuration immediately. Model
  discovery and Codex reconfiguration run in at most one pending future per
  instance, polled alongside registry requests. New saves supersede queued work; old results cannot replace
  current models or failures. Each instance stays unavailable while
  refreshing, and reports a failure if discovery fails. Per-instance
  ordering also prevents older Codex reconfiguration from running after a
  newer one. An added instance stays unavailable until its CLI has
  started, which runs beside registry requests the same way. Instances in
  the stored roster take the same launch path and probe independently while
  the registry restores sessions. The host serves after session recovery.
  A restored session takes its adapter on its first accepted relaunch. The registry derives launch settings, sign-in and the instruction
  file a CLI reads from its plugin.
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
revert turns, shut down) and waits for its
`Outcome`; meanwhile the agent reports `Step`s, among them `Step::Emit`
of an `agents::Output` (the assistant and reasoning text, streamed and
finished, a tool call, an error), which the host logs as
`EventKind::Agent`. The handle is
`runtime::Runtime`, a channel pair.

The daemon knows each provider only by the `runtime::Plugin` it is handed,
in a `daemon::Plugins` map keyed by `Provider`.

- `profile: agents::Profile`, data from the provider's core
  (`claude::profile`, `codex::profile`): the `Provider`; its
  `agents::ProviderInfo` (the label the app shows, a note for the
  new-session form, and the credential forms of §6.3); the pinned build
  (selected by `agents::Provider`); the config its private home starts with; and the
  `--version`, sign-in check and `login` runs as `agents::Invocation`s.
- `settings: runtime::Settings`: the CLI path and the model the command
  line gave it.
- `instance: fn(runtime::Launch) -> runtime::Instance` builds the account's
  provider instance with its CLI path, private home and work directory.
  It configures that account and starts its conversation runtimes.
- `account: fn(&str) -> Option<String>`: reads the account a signed-in
  home is from what the CLI reports where the profile's `account`
  (`agents::Reported`) says: Claude's `auth status` JSON (`email`), Codex's
  `auth.json` (the ID token's `email`, else `account_id`).

**Instances.** The host doesn't run providers; it runs *instances*, each
one account of one provider, such as Claude through a gateway beside
Claude on a subscription. An instance is keyed by `host::registry::InstanceId {
provider, account }` on its host (the node, docs/home.md §2): the
provider names the account, not the owner, so the key never changes and
an instance is never renamed. `agents::Account` is `Named(String)` as the
provider names it, or `Own`: the provider's own sign-in before it has
named an account. The instance itself is `host::registry::Instance { credential,
model, cli }`; the host keeps them as `host::registry::Roster`, in the credential
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

The host builds each instance through its provider's plugin, passing
its own `Launch` with its private home `<data>/home/<dir>` and its work dir
`<data>/work/<dir>`, where Claude lists its models and Codex's app-server
runs. `<dir>` is `host::registry::dir`: the provider's name for its own
instance, and otherwise `<provider>-` and the first 16 hex digits of the
account's SHA-256, which is safe in a path whatever the account is. So
each instance has its own CLI sign-in and config, its own agents and
credential, and each Codex instance its own app-server (below). Whatever
is per account is keyed by `InstanceId`: the registry's launchers,
availability, models, refreshes and sessions, `Welcome.instances`,
`Session::Start { instance }`, and `SessionCreated { instance, .. }`,
so `Continue` runs on the session's own instance. What the plugin
describes stays keyed by `Provider`: the catalog's label, note and
credential forms. The app shows an instance by its account, and the own
instance by its provider's label. `SetCredential { provider, credential }`
and `Instance::Remove { instance }` change the instances from the app; each
answers `Accepted`, once a credential change has refreshed
the instance and once an added instance's CLI has started.
`Instance::Remove` is refused with `InstanceInUse { sessions }` while a
session runs on the instance (created or ready, and not closed since):
stopping a running agent on someone's behalf would lose its turn. The
instance's closed sessions stay and can't continue, and its home stays,
since their transcripts live in it. A provider's own instance stays.

At startup and after a credential change, the host configures the instance
with its stored credential or the CLI's own sign-in. Each adapter applies
its provider's credential and discovers the models it offers. Claude probes
with a fresh CLI process, and Codex lists models on the account's app-server.
After recovering its sessions, the host accepts connections while each stored
instance starts independently. An early `Welcome` shows which instances
are still starting. Its subscription carries updated `Feed::Instances`
snapshots, starting with the current roster at subscription. A ready instance
can start or continue sessions immediately.
The instance reports credential refusal separately from discovery failure.
At boot, a discovery failure still allows the default model. After a change,
the instance stays unavailable until a refresh succeeds. The host chooses
the credential's named model, then its configured model, then the CLI default.
The daemon makes each private home from the profile (§6.2), runs the
probes and installs as `host::start` decides, and sends the plugins'
`ProviderInfo` to clients as `Welcome.catalog`.
`host::session` and the daemon's session actor know no provider, and
`crates/daemon` depends on no provider crate outside its tests.

- `agents`: the provider-agnostic API: the runtime protocol, `Step`,
  `Reply`, the native request and turn ids, `Exit`, the
  `Note`s a core logs through its shell, a provider's `Profile`, the
  runtime lifecycle that gates calls and releases closed agents, and
  the command, input, effect and failure types for CLI drivers
  (`agents::driver`). Pure `no_std`.
- `claude` and `codex`: each provider's wire (stream-json, JSON-RPC), its
  mapping into `agents`, and the state machines that drive it: the protocol
  driver, the runtime that turns commands into calls and events
  into steps (`runtime`), and Codex's `hub`. Each also holds its profile
  and its credential forms, and reads its credential from the generic
  values. Claude's resume fallbacks
  live in `claude::runtime`: a missing conversation with no message seen
  starts fresh.
  Pure `no_std`; they log nothing themselves.
- `runtime`: the session handle, provider instance, `Plugin`, and the agent
  task that feeds commands and provider events into the runtime machine.
- `claude-cli` and `codex-cli`: supply each CLI's handshake, driver and
  stderr policy to the process runner, perform each agent's provider calls,
  and expose their plugin constructors (`claude_cli::plugin`,
  `codex_cli::plugin`).
- `link`: the client link: its frames (`Open`, `HelloReply`,
  `CommandResult`, `Feed`), `ALPN`, its JSON framing, and the rules of a
  connection: admission, what each stream carries, which commands keep a
  receipt and how retries share it, the `Welcome` and the catch-up from a
  `Subscribe`. It depends on the domains and `host`; nothing below it
  depends on it. Pure `no_std`.
- `daemon`: the host behind `orchestrator`, a library that takes the
  plugins and exposes its start, serve and login functions. It runs
  `link` on each connection.
- `cli`: the `orchestrator` binary, the one place that names the providers.
  It parses the command line, builds each provider's plugin with its
  settings and calls into `daemon`. Its flags are generic:
  `orchestrator host [--cli <provider>=<path>]... [--model
  <provider>=<model>]...`, parsed in `host::start`, and `orchestrator login
  <provider> [--account <account>]`, which picks the instance of that
  account, or without one the provider's own (`host::registry::picked`).
- The private Codex hub owns its instance's `codex app-server` and
  routes each thread's events to its agent's runtime. The state machine
  `codex::hub` makes the routing decisions. Each instance has its own
  hub because the key is fixed in its server's environment.
- `install`: the shared pinned installs, CLI homes, sign-in over pipes
  (`install::process`), and the line-JSON process runner
  (`install::line_json`). The runner owns each child's pipes and executes
  its driver's effects in order. The agent task reuses the same loop to
  process each effect's feedback before the next effect. Shutdown
  interrupts a blocked stdin write and closes the pipe without writing
  any further frames. Claude's
  handshake can read the last stderr line on failure. Codex's driver gets
  the last nonempty stderr line with the exit status.

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

### 5.3 Durability, worktrees and snapshots

- **Store (built):** `<data>/conductor.redb`, with typed redb table constants
  and postcard values (storage option C). Its `metadata` records the log
  format, 8 since sessions derive their provider from their instance instead
  of storing it twice. A database of another format doesn't open and is moved aside
  to start over. The tables are `events`,
  `command_receipts`, `conversations` and `metadata`.
  Sessions, their model choices and requests are reconstructed from
  the event log; the native conversation id has its own table. A cursor is
  `(epoch, seq)`; the epoch is created with the database, so wiping the
  database makes clients start over. The ticket token and host key also
  persist, so an existing ticket remains valid after restart. Streaming
  deltas stay live-only.
- **Idempotent commands (built):** receipts are keyed by fleet-unique
  `CommandId`s (random UUIDs), so a hedge onto a new transport identity
  still finds the same receipt. One command at a time stages its durable
  events and conversation writes, then commits them with its receipt
  before replying. Ordinary commands use one transaction. Snapshot capture
  keeps enough durable intent to recover work left by an interrupted turn.
  A retry with a completed receipt returns its stored result. If
  execution is cancelled or a write fails, the log rolls back only its
  uncommitted suffix and the host stops its transport and provider processes.
  Previously committed capture intent remains pending for recovery. Provider
  stdin writes and the separate credential file are external effects: a
  crash before the database commit can leave a CLI action sent without a
  receipt. Restart withdraws its requests and interrupts its turn; it does
  not promise exactly-once execution inside a CLI.
- **Credential receipts:** the instance commands (`SetCredential` and
  `Instance::Remove`) reserve their command id without holding
  the global event transaction during an instance's refresh or start.
  Other commands and live events can proceed. Duplicate ids wait for the
  original result; superseded saves wait for the newest refresh. The final
  `Accepted` result is committed as the receipt before any reply, after an
  active ordinary transaction finishes.
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
- **Worktrees (built):** new sessions require a committed
  Git repository and get `<data>/worktrees/<ulid>` on `conductor/<ulid>`,
  initially from committed HEAD. `host::workspace::Plan` decides the whole
  workflow for creation, capture, diffs, removal and landing.
  The daemon executes each Git or filesystem step and passes the result
  back before the plan issues the next step. The original working tree and index stay
  untouched. The selected subdirectory is resolved within the managed tree.
- **Commit snapshots (built):** the initial workspace state and each
  finished, failed or interrupted turn record a commit from the session's
  branch (home §2.3). Capture uses a temporary index based on the branch
  tip, adds the workspace's files and writes their tree. If that tree
  differs from the tip, `commit-tree` creates a commit on top of it and
  a compare-and-swap updates the branch. Otherwise the snapshot reuses
  the tip. Commits the agent made during the turn stay in the branch's
  history. The worktree's index follows the branch while its files stay
  in place; staging choices separate from the files are not retained.
  Opening events are `SessionCreated`, `OnIssue` for an issue start,
  `WorkspaceCreated`, and `SnapshotRecorded`. The client link's `Snapshot`
  is only a commit and an optional `turn_start`; the initial snapshot has
  no turn. Each recorded snapshot event has its existing `HostSeq`, so
  two unchanged turns remain separate records even when they share a
  commit. There is no separate snapshot id, reservation, ancestry or
  provider-history anchor. Git owns the commit history; the session log
  records which workspace state belongs to each turn.
  A retention ref at `refs/conductor/<ulid>/<commit>` keeps each recorded
  commit reachable and binds it to the session for diff requests.
- **Capture recovery:** an acknowledged message leaves durable intent to
  capture its workspace even if the provider's turn-start notification is
  lost. Failed captures remain pending across a restart. Send and Continue
  retry capture between turns before accepting more work, even if the
  agent is already running; an unsuccessful retry rejects the command.
  A retry captures the current files against the current session branch,
  without resetting it to an earlier snapshot. Branch updates check the
  expected tip, so a concurrent or later commit cannot be silently dropped.
- **Removal (built):** `Session::Remove` is refused while a turn runs. It
  stops a running agent (`SessionClosed`), then follows
  the workspace removal plan: commit the worktree's untracked and changed
  files onto `conductor/<ulid>` with `commit-tree` and `update-ref` (files
  Git ignores, such as build output, go with the worktree), push
  only that branch to `origin` (never forced; a failed push keeps it
  locally and goes on), `git worktree remove` (or `prune` when the
  directory is gone already). Snapshot retention refs remain because their
  recorded events survive removal. A
  failed commit or remove stops the removal, logs an `Error` and keeps the
  session. The
  branch stays in the checkout. `SessionRemoved { kept }` ends the log of
  the session, which then refuses every command with `SessionRemoved`.
- **Per-turn diffs (built):** `GetDiff` names the snapshot's event sequence
  in its session. The host finds that record and the preceding recorded
  snapshot, checks their retained commits belong to the session, and
  compares those commits. It never substitutes the selected commit's Git
  parent: a turn can contain several agent commits, and unchanged turns
  can share a commit. The initial snapshot provides the first turn's
  baseline. Diffs have bounded file counts and patch sizes; binary and
  oversized files get markers. `GetDiff` bypasses durable receipts so
  patches are never copied into the event log or receipt table.
- **Diffs only read.** Opening a turn's diff changes no files and no
  conversation; provider histories are never rewound or forked, and
  snapshot recovery only captures workspace state. Ordinary Continue still resumes the saved provider
  conversation, including after a host restart. Finishing and removal
  keep the session's branch commits.

### 5.4 The home and its projects (built)

The home (`docs/home.md`) is the owner's git repository of projects,
standards and issues, cloned on every host. Built so far are home steps
3, 4 and 6: the clone, its sync, each project's checkouts, and issues.

- **The home's files.** `projects/<id>/project.toml` lists each project
  under a folder named by its id, 6 random lowercase base 32 digits, which
  is the `ProjectId`. It gives its `name`, unique in the home and what a person
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
- **The clone.** `orchestrator host --home <remote>` gives the host the
  home, which it stores in `<data>/home.state`, so later starts need no
  flag. One task, `daemon::home::HomeClone`, owns the clone in
  `<data>/home-repo` (`<data>/home` holds the instances' private homes) and
  runs plain `git`, as the workspaces do. It clones when there is no clone
  and otherwise pulls (`git pull --rebase`, which fast-forwards; a rebase
  that stops is aborted) every five minutes, on `Home::Sync`, and before a
  read that needs it (a session started in a project, a clone) unless it
  tried within 30 seconds. A failure is state, not an error:
  `home::HomeSync` is `Syncing` (first clone), `Synced`, `Behind` (a
  clone, but the last pull failed) or `Failed` (no clone), with
  `Home.synced`, the last successful sync. What the home looks like now is
  on a watch channel, so `Welcome.home` never waits on a pull, and each
  change reaches clients as `Feed::Home`, which replaces the last.
- **Checkouts.** A local repository is a project's checkout when its `HEAD`
  history has the project's root commit and its `origin` is the project's
  remote, compared without scheme, user, `.git` or case; a local-only
  project's checkouts have no `origin`. The host probes, in order, the
  checkouts it found before (host-local, in `<data>/home.state`) and its
  own clones, then in each search root (`--projects <dir>`, repeatable)
  each directory and each directory in those, skipping hidden ones and
  those in `_old` or `Archives`, running in each what
  `home::checkout::ASKED` lists (`rev-parse --show-toplevel`, `rev-list
  --max-parents=0 --first-parent HEAD` and `remote get-url origin`) with
  its time-limited Git runner; the decisions and the reading of Git's
  answers are `home::checkout`, the walk is `home-files`, and `home repos`
  does the same with the home's folder as its one root, running Git with
  std's processes. A project may have several checkouts on a host
  (`home::Checkout { path, origin: Found | Cloned }`, its path a
  `home::CheckoutLocation`), kept by path in `<data>/home.state`. `Home::Clone` clones a project with a remote into
  `<data>/projects/<project>` (`Project.cloning` is `Running`, then `Idle`
  or `Failed`). The host only reads the checkouts it finds; sessions run in
  worktrees made from them (§5.3).
- **Sessions in a project.** `Session::Start { at: StartAt::Directory |
  StartAt::Project }`: a project starts in its first checkout, or is
  refused with `NoHome`, `UnknownProject` or `NotCheckedOut`. The session
  stores no project of its own: its worktree's repository, which
  `WorkspaceCreated` records, is the checkout, and the app finds the
  project from it (home §2.2).
- **Issues** (home step 6). `projects/<project>/issues/<id>.md`, one file
  per issue in the format of home §3, its name the issue's id (`IssueId`):
  8 lowercase base 32 digits, the seconds since 2026-01-01T00:00:00Z and 2
  random, so they list in creation order; `home::issue` reads it and lists a project's issues in
  `Project.issues`. The app and the `home` command line share one edit
  vocabulary, defined in `home`: `CreateIssue { edit, issue: NewIssue }`
  is `home issue new` (project, kind, title, `<field>=<value>` fields),
  with an id the host mints, and `ChangeIssue { edit, issue, change }`
  makes one
  `IssueChange`: a `Move` of the stage or status with a reason, a `Set`
  of a field (the title among them), a `Note`, a new `Body`, or a
  `Checklist` item added, or checked, unchecked or removed by its place
  in the checklist shown to the person. The host writes each through
  `home::flow::command::write`, as the command line does: the file is
  read back against the flow among the home's other issues, a structural
  problem or an `error` rule refuses it (`IssueRefused`, with the command
  line's reason), and anything softer is written and answered with the
  warning (`IssueWritten { issue, warnings }`). Its log lines are signed
  `<host>·<client>`. A change rewrites only its own lines of the file, so
  comments, keys, tables and sections a person added stay as written
  (Windows line ends become Unix ones). Both commands use
  `link::log::Kind::Receipt`, so a retry returns the saved response. A local
  storage failure returns `IssueNotSaved` without keeping a receipt, so
  the same edit can retry when storage recovers. An edit is known by its
  `EditId`, which the app mints once per edit and sends again with each
  retry under a new command id, so the home applies it once. When the host dies after safely recording a new
  issue and before committing its receipt, a retry recovers its original
  ID from the queue or the commit that added the issue file, even if the
  current document is missing or malformed. Before acknowledging a
  change, the host saves its edit id and the previous field value in
  `<data>/home-edits.state`. The host writes and commits the issue file
  locally, then attempts to push. If the queue cannot be saved, the
  command returns `IssueNotSaved`; if a later commit or push fails,
  the saved edit can be replayed. When an earlier edit to the same
  issue is still waiting, the host saves the later edit for replay too.
  A refused push or a sync with pending edits fetches and resets the
  clone to upstream, replays the pending edits in order, then pushes.
  If the fetch fails, the host replays against its local commits so
  subsequent edits can appear immediately while offline. A queue stored
  in an older version is set aside unread, never converted.
  Replay uses the fetched upstream file's last commit time to resolve
  conflicting fields. The issue file records which logical edits have
  been resolved, including edits whose value was already present and
  those superseded by a newer value. Those outcomes can produce a commit
  that adds only an identity marker.
  An unreadable or malformed issue holds its edit and later edits to
  the same issue. An absent issue can be created if its path has no
  upstream history. Confirmed deletion lets the host retire its edits.
  After a successful push the home core keeps any edits that still
  wait, and the daemon saves that shortened queue before adopting it.
  If saving fails, the host can replay the older queue without repeating
  a resolved edit. `Home.unpushed` counts the kept edits.
  The host retries reading a temporarily unreadable queue before saving
  new edits and moves an undecodable queue aside to
  `home-edits.unreadable`.
- **Sessions on issues** (home step 7, started). `Session::Start { at:
  StartAt::Issue(issue) }` starts in the issue's project's first
  checkout (`NotCheckedOut` without one). An instance has one session on
  an issue until it is removed (`IssueTaken`),
  checked before anything else. Then the host readies the issue's branch,
  `issue/<id>`, in the checkout (`home::issue::Branch`): it fetches
  `origin`'s, the remote the home matched the checkout by, into
  `refs/remotes/origin/issue/<id>`, and points the checkout's at it when
  the checkout has none or one the remote's contains; one the remote's
  lacks commits of stays. A new one starts at `origin/HEAD`, else the
  checkout's `HEAD`. It refuses (`WorkspaceFailure`) to make a new branch
  while `origin` can't be read, and to start when both have moved on,
  so a session never leaves the remote's work out. The session's own
  branch is cut from it, `OnIssue { issue }` follows `SessionCreated`, and
  the agent's first message is the issue's brief (`home::issue::start`):
  its title, body, checklist and notes, its stage and the skills that
  serve it, the branch, and what to do when done; one that doesn't reach
  the agent leaves an `Error` in the session. The home actor then moves
  the issue to the flow's first status of role `active`, unless it is
  active already or the move breaks one of the flow's rules, as the issue
  is then (`home::issue::SessionUpdate`). The fetch runs in the
  home actor, so issue commands wait for it. The app's issue pane lists
  its sessions on every paired host and opens the new-session form on it.
- **Finishing** (home step 7). `Session::Finish { session }`, for a session
  on an issue between turns (`NotOnIssue`, `TurnInProgress`) that has done
  something since it last finished (`AlreadyFinished`; its own summary
  turn doesn't count, `host::finish::FinishState`), keeps a receipt, logs
  `FinishStarted` and answers `Accepted`. The session actor drives
  `host::finish::Finish`. It asks a running agent for one summary turn
  and then stops it. A stopped agent's last reply stands in. If asking
  or stopping fails, the finish lands nothing. The actor continues
  handling runtime events and ordinary orders while following the log
  for the summary turn's end. It replays from the same cursor if the
  subscription falls behind, keeping the original ten-minute deadline.
  On timeout it interrupts the summary and records a failed finish.
  The host brings the issue's
  branch up to `origin`'s first, as a start does, refusing when both have
  moved on; commits what the worktree holds onto the session's branch;
  then lands it on the issue's in the checkout
  according to the workspace landing plan: nothing when the issue's has it all, a
  fast-forward, or a merge commit made with `git merge-tree` without a
  worktree, whose answer the plan reads; a conflict, named
  by its files, lands nothing, and neither does an issue branch
  checked out somewhere, which can't move under its worktree. It pushes
  the issue's branch to `origin`, never forced, which lets another host
  continue the issue. The summary becomes the issue's note, signed
  `<host> · <instance>`, and an issue still `idle`, `startable` or `active`
  by role whose branch gained commits moves to the flow's first status of
  role `review`
  (`home::issue::SessionUpdate`). `SessionFinished { finished }` ends it: the
  commits that landed and where the branch went (`Kept`), or why nothing
  landed. The session stays, closed; *Remove* removes its workspace.

## 6. The fleet

The goal is one setup running in several places. Every host runs the
Claude Code and Codex versions pinned by its Conductor release, installed
and owned by `orchestrator`. The fleet shares credentials, except a Codex
ChatGPT login, which each host signs in to once (§6.6). Model defaults are
local to each host and instance; sessions can choose their own models.
A host that joins the fleet can run independent sessions without manually
copying the shared credentials.

D4 still holds. A session belongs to its starting host for its lifetime,
and its events are in that host's log. What the fleet shares is configuration, not sessions.

Facts about the CLIs and their terms are marked **verified** (read in the
cited source, or observed on the developer's machine) or **inferred**. They
were checked on 2026-11-12, against Claude Code 2.1.316 to 2.1.322 and
Codex 0.166 to 0.167.

### 6.1 Trust and pairing

Today a ticket's random token stands in for a device certificate. The fleet
replaces it.

- A **fleet root key** (Ed25519) signs `DeviceCert { fleet, device, name,
  role }`. The role is `Owner` for a browser and `Host` for an `orchestrator`.
  Hosts store only the root's public key and check the certificate in
  `Hello` against it, the authenticated iroh node id and the revocation
  list in the fleet config (§6.5).
- Hosts talk to each other with the same certificates, over iroh, on their
  own ALPN `conductor-fleet/<n>`. Clients never open it.
- The root key lives on one host, the **root host**. It is the first host,
  which creates the fleet with `orchestrator fleet init`. It signs device
  certificates and every fleet config version, and the other hosts hold no
  signing key. `fleet init` also writes the root key once as a
  passphrase-sealed file for the owner to keep, so a lost root host doesn't
  strand the fleet (F1, §6.10).
- A browser keeps its device key as a non-extractable WebCrypto key in
  IndexedDB, and iOS gives a Home Screen app its own storage, so the phone
  pairs from the installed app. The app shows its public key as a QR code
  and `orchestrator pair` on the root host signs it. A paired app can approve
  a new one the same way.
- Whoever controls the GitHub Pages deploy controls the code the browser
  runs, and from §6.3 on that code carries credentials in flight. Protect
  the deploy branch, use Subresource Integrity, and keep 2FA on.

### 6.2 The daemon manages its own CLIs

`orchestrator` installs a pinned version of each CLI into its data dir, checks
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
   `cosign` is installed. Each writes only its provider's
   `crates/<provider>/src/pin/pins.rs`, including the verification result as
   its `VERIFIED`. Versions, platform hashes and verification provenance stay
   together in compiled Rust values, beside the provider's `pin`, which says
   which build a platform takes.
2. `orchestrator` compiles the pins in. On start it makes sure the pinned CLI
   is at `<data>/cli/<provider>/<version>/`: `claude` there, and Codex's
   package unpacked whole, with `bin/codex`. It downloads to a temporary
   file, checks the SHA-256 of that file as written to disk, and renames it
   into place (the package is unpacked with the system `tar` beside it
   first, then the directory is renamed), so a crash leaves either nothing
   or a verified install. It holds a lock on `<data>/cli/<provider>/lock`
   throughout, so a second process on the same data dir, such as
   `orchestrator login` during a first start, waits and then finds the CLI
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

Deploy that tested Conductor release to each host to update its CLIs.
Fleet configuration does not carry pins or provider settings, so there is
one normal installation path for standalone and fleet hosts. Existing
explicit CLI paths for tests and development still override the pin.

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

`orchestrator` writes `<data>/home/claude/settings.json` (`cleanupPeriodDays`
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
`orchestrator login <provider> [--account <account>]` does the same on the
host's terminal: both run the CLI's own sign-in against the instance's
private home. Both require a successful login and status check before
resolving the account. If the CLI reports another account, its instance
takes the home and work directory (§5.2). Adoption records which original
directories it staged before installing them, so recovery can finish the
roster update even if another launch creates a fresh source directory.
The previous destination copies are kept until the roster is durable.
`login` stores the result for the host's next start. Both run the
provider's `claude auth login --claudeai`, §6.3's per-host mode, which
stores its own credential, or `codex login --device-auth`. A private home
signed in this way counts as configured (`Auth::SignedIn`, §6.3). The host
checks it with `claude auth status` and `codex login status` (15 s at most
each) when it starts and again after a sign-in from the app, so a sign-in
from the app shows at once, while one made on the terminal while the host
runs, or one that expires, shows only after a restart.

WSL1 does not support Claude?s command sandbox. The host reports that commands
can read the gateway token when the sandbox is unavailable.

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

No client link or store type names a provider's credential. Each provider's
plugin describes the kinds it takes in its `ProviderInfo`, which
`Welcome.catalog` carries: a `CredentialForm { kind, label, fields,
account }`, whose `account` names the field that is the account (§5.2), and
each `Field { id, label, kind, placeholder, required }` of kind `Text`,
`Url` or `Secret`. The app draws each form from that description, knowing no
provider, and a credential travels and is stored as
`Credential { kind, values }`, its form's values by field id;
`SetCredential` names the provider, and the account field's value the
instance.
The host checks it against the form (`ProviderInfo::check`): the kind is
one the provider takes, every value is a field's, a required field is not
empty, and a `Url` field is plain `http(s)://host/path`. The provider's
core reads its values into its own type (`claude::ClaudeCredential`,
`codex::CodexCredential`) and decides what they become: environment,
config edits and the default model. Another kind is another form in the
provider's core, and the daemon, `host`, `link` and the app
don't change.

#### Built first: gateway credentials

The owner reaches both models through gateways, so their forms remain first in the credentials screen.

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
  the host log "the CLI's commands can read the provider credential; install
  bubblewrap and socat to sandbox them" with `provider=Claude`
  at start and when the gateway is set. The base URL reaches commands either way; it is an address, not a
  secret. A live canary had the agent count the token in a command's
  environment: 0 with the sandbox, and 1 without it (observed both). A change applies to the
  next session. Clearing a Claude credential
  doesn't stop the sessions already running on it, unlike Codex below; to
  cut off a leaked token, revoke it at the gateway.
- **Codex:** the form `provider`, with a `Provider name`, a `Base URL`, a
  `Model` and a `Key` (a secret), read into `CodexCredential::Provider {
  name, base_url, key, model }`, whose model is the provider's default.
  When applying the credential, the Codex adapter writes
  `[model_providers.conductor]` (`name`, `base_url`, `env_key =
  "CONDUCTOR_CODEX_KEY"`, `wire_api = "responses"`, the only one Codex
  0.166 knows) and `model_provider` and `model` into the private
  `config.toml`, keeping the rest of the file. Failure to apply the edits
  refuses the credential before discovery. The app-server receives the key in
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
  and discovery starts a server with the new key after the private config
  has been updated.
- **Protocol:** `SetCredential { provider, credential }` and
  `Instance::Remove { instance }` answer `Accepted` when the command completes.
  A retry returns its original receipt even after the offer has changed.
  Clients follow the current roster through `Welcome` and `Feed::Instances`;
  neither command results nor sign-in completion carry snapshots. Completion
  and roster updates can reach the client in either order.
  These commands require a client admitted by the ticket's token, which
  the device certificates of §6.1 replace.
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

#### Direct API keys

Each provider offers an API key form beside its gateway form. A public account
label names the instance, and the API key is a write-only field. Entering the
same label again updates that instance. The key follows the existing encrypted
credential store and can be carried by the fleet configuration when #14 lands.

- Claude passes the key to the session in `ANTHROPIC_API_KEY` with the
  Anthropic endpoint. The same sandbox-dependent subprocess scrub used for
  gateway tokens applies.
- Codex selects the existing custom Responses provider with the fixed OpenAI
  endpoint `https://api.openai.com/v1`. It passes the key through
  `CONDUCTOR_CODEX_KEY` and keeps the shell environment exclusion. The
  direct form leaves model selection to Codex's catalog and session options.
  The key does not become a saved Codex login.

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

A token minted by the pinned CLI can be entered using the app's **Claude
setup token** form. Its public account label identifies the instance. The
credential store encrypts the token using the same path as API keys. It
reaches the CLI as `CLAUDE_CODE_OAUTH_TOKEN` at session startup. The
credential kind and values fit the instance roster carried by #14. Fleet
distribution and the issue-time and renewal metadata are separate work.

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
   instance of yet. The app sends `SignIn::Start { instance }`, and the host
   runs the
   instance's CLI sign-in in its private home, its stdin, stdout and
   stderr on pipes. A host runs one sign-in per instance at a time and
   refuses a second start (`SignInRefusal::SignInRunning`) until the first ends
   or is cancelled, so two devices never race one CLI; *Cancel*
   (`SignIn::Cancel { instance }`) kills it, as do a 15-minute limit (Codex's
   one-time code expires then) and the host stopping.
2. **What the CLI prints is read as its plugin describes.** Each provider's
   core gives an `agents::SignIn` in its `Profile`: the run; where the
   output shows the link (`Pick { after, word: Link }`, the first `https://`
   word after that text) and any code to enter on the page (`word: Code`,
   six or more capitals, digits and dashes); whether the page gives a code
   the CLI reads back on stdin (`paste`); and what a failure the CLI prints
   means (`Failure { says, means }`). It exits 0 when it succeeds. The host
   strips terminal colours and keeps the output only until the link is
   there. The daemon, `host`, `link` and the app read only
   that description.
3. **Claude, per-host.** `claude auth login --claudeai` on pipes prints
   "Opening browser to sign in…", then "If the browser didn't open, visit:
   <url>", whose page "shows a code instead of returning to `localhost`"
   ([troubleshoot], verified), then waits at "Paste code here if prompted >"
   (observed with 2.1.316, `fixtures/claude/sign-in.txt`). The app shows
   *Open the sign-in page* (a new tab) and a password field; the code goes
   in `SignIn::SubmitCode { instance, code }` and the host writes it to the
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
   and not observed. If device code authorization is off, the app points to
   ChatGPT Settings ? Security. Where that setting is unavailable, the
   OpenAI API key form is available from the phone. For a terminal
   fallback without device authorization on a headless host, use Codex's
   browser login in the instance's private `CODEX_HOME` with an SSH
   tunnel to port 1455 (?6.6). The app-server's `account/login/start { type:
   "chatgptDeviceCode" }` would do the same; the CLI keeps one mechanism for
   both providers.
5. **After it.** A sign-in that exits 0 runs the CLI's sign-in check against
   the home; one that passes makes the instance sign in on its own
   (`OwnSignIn::SignedIn`) and refreshes it, which lists its models. The
   last step, `SignedIn`, reports completion. Subscribers receive the current
   availability through `Feed::Instances`. A CLI the host was given (`--cli`)
   is checked the same way after a sign-in from the app, though it isn't at start.
6. **Claude setup token from the app.** The owner gives the credential a
   public account label and chooses **Sign in to create a token**. The host
   runs the pinned CLI's `setup-token` in a private pseudo-terminal. On the
   paired phone the owner opens Claude's link, then pastes Claude's one-time
   return code into Conductor on that same phone. The host sends it to the
   waiting CLI. After Claude reports success and prints the long-lived
   token, the host saves it through the credential store and refreshes the
   named instance. The phone receives the instance status. The per-host
   login still runs on pipes. Multi-host token use awaits E2 (§6.9), and
   early revocation remains unverified.

**Live only, stored nowhere.** The steps, `Starting`, `Open { url, code,
paste }`, `Finishing`, `SignedIn`, `Failed { reason }` and
`Cancelled`, reach clients as `Feed::SignIn` frames on the subscription
beside its events. They are not events: the log neither stores nor replays
them, and a new subscription starts with the sign-ins running then, taken
in the same registry turn as the feed of later steps. The sign-in commands
keep no receipt (`Route::ledger()` returns `None`), and their answers are `Accepted`
or a rejection, so nothing in the ledger holds a code. The command log
records the operation and command ID on receipt, and the ID and result
category on completion. The host logs each step's name and a failure's
reason, never the link, the output or a code. The end-to-end test signs in with a wrong and a right
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

- On macOS the host key lives in the user's default Keychain, typically
  the login Keychain. On Windows it is a user-private generic credential
  in Credential Manager with local-machine persistence. On Linux it
  goes in the user's unlocked default Secret Service collection over an
  encrypted D-Bus session. Where no keystore is usable when the store
  is created, or `<data>/store.key.file` asks for it, the key is the
  owner-only file `<data>/store.key` (mode 0600 on Unix) instead. A store
  keeps the place chosen at its creation; one in a keystore requires that
  entry, even if the keyring is temporarily unavailable. The separate
  `<data>/host.key` and `<data>/auth.key` still hold the iroh and pairing
  keys. See [credential store backup and recovery](credential-store.md).
- The credentials are one value, `host::registry::Roster`, the host's instances
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
  `Instance::Remove` for any of them moves it to
  `<data>/credentials.unreadable` and starts a store of the own
  instances, which is the recovery from the phone. A missing or
  inaccessible keystore entry stops the host before it opens the
  encrypted store, and so does an encrypted store without its key file.
- A Claude environment credential is kept from Claude's commands only where
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
- API keys reach the CLIs in their environment at spawn. Claude uses
  `ANTHROPIC_API_KEY`, and Codex uses the shell-filtered
  `CONDUCTOR_CODEX_KEY` with its configured Responses provider. The private
  CLI homes contain config and conversation state. Codex's per-host ChatGPT
  login keeps its own refreshed tokens in the private home.

### 6.4 Join a host to the fleet

A host joins with a one-time invite, from either side.

- **From the app.** *Add host* asks the root host for an invite and shows
  `orchestrator join <ticket>`. The ticket holds the root host's node id, its
  home relay and a random 32-byte invite secret that expires after one hour
  and works once. Running the command on the new machine dials the root host
  on `conductor-fleet`, proves the secret, and receives a `Host` certificate
  for its node id and the current fleet config.
- **From the host.** An `orchestrator` started with no fleet prints its own
  ticket, as it does today. Scanning it in a paired app sends its node id,
  relay and token to the root host (`FleetCommand::Admit`), which dials the
  new host and admits it the same way; the new host's ticket token is its
  enrollment token (`host::fleet::EnrollmentToken`).

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
    instances: Roster,            // with their API keys and Claude token; sent only to hosts
    project_credentials: Map,     // named repository credentials shared with hosts
}
SignedFleetConfig { config: FleetConfig, signature: [u8; 64] }  // by the root key
```

- **One writer.** Only the root host signs, so versions never fork and there
  is nothing to merge. A change from the app (a new credential or a
  new host) is a command to the root host, which applies it, bumps the
  version, signs and stores it.
- **Any host serves it.** Every host keeps the newest valid config it has
  seen in its store. When two hosts connect on `conductor-fleet`, each sends
  its version; the one behind asks for the document and checks the signature
  and that the version is higher before it adopts it. The root host also
  pushes each new version to every host it can reach.
- **An offline host catches up** the next time it reaches any other host,
  whether that is the root host or not. A host that comes back after a long
  time adopts the newest version, applies shared credentials and drops revoked
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

The fleet exchange uses `conductor-fleet/5`. Full configuration signatures,
local sealed state and root backups use version 5 contexts. A certificate
and the public snapshot are signed over their exact JSON bytes, under the
version 2 certificate and snapshot domains, and carried as those bytes
(`host::fleet::Signed`): JSON has no canonical form, so every verifier,
hosts included, checks the signature over the bytes and only then parses
them. An unreadable `fleet.sealed`
stops startup rather than switching the host to ticket authentication.
Previous state and backups are set aside before initializing and enrolling
a fresh fleet, as described in `docs/fleet.md`; no old format is decoded
or converted.

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

`SessionId` is a fleet-unique ULID (`crates/host/src/session/record.rs`). The app
shows independent sessions from every host, grouped by host.

- **Format.** A `SessionId` is a 128-bit ULID: 48 bits of Unix milliseconds
  and 80 random bits, generated by the host that creates the session. Stores
  carry its `u128` value with postcard's variable-length integer
  encoding; as text, and so on the client link, it is the 26-character
  Crockford base32 form the `ulid` crate writes, such as
  `01JCDZ6Q0Y3K7V8M2N4P5R6S7T`, as in its branch, worktree and logs. No
  coordination between hosts is needed, because two ids collide only if
  they share the millisecond and all 80 random bits. Ids sort by creation time, which the sidebar uses.
- **Branch.** `conductor/<ulid>`. A session can carry a title, but the title
  is not in the branch name, so renaming never touches git.
- **Worktree.** `<data>/worktrees/<ulid>`.
- **Snapshot retention.** `refs/conductor/<ulid>/<commit>`, as in §5.3.
- **Short form.** The UI shows the last 6 characters where space is tight,
  and the full id on hover or long-press.

`HostSeq` and `Epoch` stay per host (D4). An event names its session by id
and remains in that session's host log.

The id change landed with durability, before redb tables keyed on it. It
changes the wire format. The protocol ALPN was then `conductor/10`.
The current snapshot representation and capture recovery are described in §5.3.

### 6.8 Sessions stay on their starting host

A session's native conversation, worktree, snapshot retention refs and
events stay on the host that created it. Local Continue, restart recovery
and diffs use that host's durable history and provider files.

To continue an issue elsewhere, finish the current session to land its
work and summary, then start a new session on the other host. That host
uses the project's remote branch and the issue's brief and notes; it starts
a new conversation. A project without a remote needs its repository
transferred separately.

Cross-host session migration was removed on 2026-11-22 to reduce the
coordination required across the host, provider adapters and client. There
is no session transfer protocol, transcript archive interface, imported
history or ownership handover. Fleet membership, pairing, credentials and
multi-host supervision remain independent of session ownership.

### 6.9 Experiments

**E1, 2026-11-21.** With pinned Claude Code 2.1.316 on Ubuntu 26.04.1
x86-64 under WSL2, `claude auth login --claudeai` printed a link on
pipes and accepted the returned code on stdin. In the isolated profile,
`claude auth status` reported signed in. After logout it reported signed
out. For `claude setup-token`, the pipe probe timed out after 120 seconds
without recognizing a link. A fresh pseudo-terminal produced a sign-in
link. On the first terminal attempt, the verifier received a return code but
could not establish issuance. On the second, the owner scanned a QR for the
manual authorization link and entered the return code. The runner observed
both the CLI's success message and the printed token after stripping terminal
colors, then closed the CLI and discarded the token. This was a CLI probe;
the code was entered on the test computer.

E2 and E3 still need live observations; `docs/verification/codex_manual_probe.py`
runs E3's refresh and turn probes in a disposable `CODEX_HOME`. E4 and E5, the cross-host resume
experiments, were retired with session migration (§6.8).

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

E1 establishes the two Claude input modes. E2 tests
the shared token and settles the fallback. E3 decides whether Codex by
ChatGPT can run on several hosts at once. Run each before the phase that
needs it: E1 before sign-in from the app, E2 and E3 before fleet and pairing.

### 6.10 Decisions

The owner settled the initial decisions on 2026-11-12; later decisions
record scope changes.

| # | Date | Decision |
|---|---|---|
| F1 | 2026-11-12 | **The root key lives on one root host**, the first host, with a passphrase-sealed backup file written at `fleet init`. Fleet changes wait while the root host is offline. Putting the root key on every host was rejected, because then one stolen laptop could admit devices. |
| F2 | 2026-11-12 | **One Claude `setup-token` token is shared across the fleet**, encrypted, through the signed fleet config, against the clause quoted in §6.3, as a grey area the owner accepts for their own account and tool. The risk is action against the account. The per-host `claude auth login` mode stays designed as the fallback (§6.6). |
| F3 | 2026-11-12 | **A Codex ChatGPT login is one sign-in per host**, because refresh tokens rotate and a reused one fails with `refresh_token_reused` (§6.6). |
| F4 | 2026-11-12 | **API keys, Anthropic and OpenAI, are shared fleet-wide** through the fleet config. |
| F5 | 2026-11-12 | **The Claude sign-in code is relayed through the app**, in flight only, to the CLI's stdin on the host (§6.3). |
| F6 | 2026-11-12 | **Codex device-code sign-in is on.** The owner enables it in ChatGPT's security settings. Without it, a ChatGPT login on a headless host needs an SSH tunnel to port 1455, which the phone can't open. |
| F7 | 2026-11-12 | **Retired by F12.** Session transfers used Git bundles over iroh to avoid remote branch and CI side effects. |
| F8 | 2026-11-12 | **Retired by F12.** Session transfers required matching CLI pins because transcript formats are internal. |
| F9 | 2026-11-12 | **Codex analytics are off** (`analytics.enabled = false`, [codex-src], verified). F13 removes the fleet override; every app-server launch supplies this setting. |
| F10 | 2026-11-12 | **Retired by F12.** Session migration was sequenced after durability, worktrees, fleet and push. |
| F11 | 2026-11-12 | **Experiments E1 to E5 may run** in the phase that needs each, in scratch config directories only, with the owner told before each run (§6.9). |
| F12 | 2026-11-22 | **Sessions belong to their starting host for their lifetime.** Remove cross-host continuation, branch bundles, transcript archives and ownership transfer to reduce coordination and interface surface. Continue work elsewhere in a new session using the issue and its remote branch (§6.8). |
| F13 | 2026-11-22 | **Provider CLI upgrades ship with tested Conductor releases.** Remove fleet pin/settings publishing and its alternate install and reconfiguration paths. Keep local model defaults, session choices, verified managed installs and shared credentials. Codex analytics stays off without a fleet override. |

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

## 7. The client (`crates/web-iroh`, `web/`)

The browser app is TypeScript, React and Effect in `web/`, over iroh
compiled to wasm (ADR 0001). The link to a host and the fold of its events
live only there. `crates/client`, `net`'s `HostLink` and the Rust fold
(`HostFold`, `HostView`) are gone. Both the daemon and the wasm pipe
use `net::wire` for framing; the daemon also uses the relay helpers
`net::without_root_dot` and `net::n0_relays`.

### 7.1 The pipe (`crates/web-iroh`)

A wasm-bindgen crate (`layer = "shell"`), the browser build entry point.
It is a byte pipe to one endpoint, and its whole surface as TypeScript
sees it is:

```ts
connect(endpoint: Uint8Array, relay: string, alpn: Uint8Array): Promise<Connection>
interface Connection { openBi(): Promise<Stream>; closed(): Promise<string>; close(): Promise<void> }
interface Stream { send(frame: Uint8Array): Promise<void>; recv(): Promise<Uint8Array | undefined>; finish(): Promise<void> }
```

- `connect` takes the host's 32-byte endpoint id and its home relay, both
  from its ticket, binds a fresh local endpoint (`presets::Minimal`) whose
  only relay is that one, and dials. The connection owns its endpoint, and
  `close` closes both; `closed` resolves with why the connection ended.
- A frame is one body, framed by `net::wire` (§7.2). `send` writes it;
  `recv` returns the next body, or `undefined` once the peer
  finished the stream between frames. Sending and receiving on one stream
  may run at once.
- It reads no body and knows nothing of tickets, authentication or
  reconnecting.
- iroh needs its `tls-ring` feature here, so the build needs clang for
  wasm32. `npm run pipe` in `web/` builds it with wasm-pack (`--target web`,
  the `wasm` profile: `opt-level = "z"`, LTO, one codegen unit) into
  `web/web-iroh`, which is not committed.
- `web/src/pipe.ts` types the app's `Pipe` from the generated declarations,
  and `web/src/test/fake-pipe.ts`, an in-memory pipe whose host side a test
  plays, is typed against the same declarations, so a change to the pipe's
  surface fails the type-check of both.

Decision 0tws30ha looked for a way to reach a host without Rust in the
browser. Every option needs a public relay of our own, so the pipe stays
on n0's relays.

### 7.2 Frames and their codecs

A frame is a big-endian `u32` body length and its body, at most 16 MiB,
and `net::wire` alone writes and reads it, refusing an oversized frame
before its body. Each link encodes its own messages in its own codec, and
a message can only travel in its link's (`link::frame::Frame`,
`host::fleet::frame::Frame`):

| Link | Codec | Encoding | Messages |
|---|---|---|---|
| Client and host (`link::ALPN`, `conductor/52`) | `link::frame` | JSON by serde | `Open`, `HelloReply`, `CommandResult`, `Feed` |
| Host and host (`host::fleet::ALPN`, `conductor-fleet/5`) | `host::fleet::frame` | postcard | `fleet::Request`, `fleet::Reply`, `fleet::SignedConfiguration` |

On a host, the daemon's `serve` and `fleet::wire` pair `net::wire` with
their link's codec; in the browser, `web-iroh` carries bodies through
`net::wire` and TypeScript encodes them.
JSON names every variant and field, so any change to a type reachable from
the four client roots bumps `link::ALPN`, and an older peer fails the
handshake. Each id writes its own JSON form, the `ulid` crate's text; an
epoch's decimal digits and maps keyed by a struct are in `host`'s `json`
module (§4.1). Host stores stay postcard.

Signed fleet payloads are `host::fleet::Signed<T>`: the exact JSON bytes
the value was written as and a signature over `T::DOMAIN` followed by
those bytes, each kind under its own domain (`conductor fleet certificate
v2`, `conductor fleet snapshot v2`). A browser certificate is
`Signed<Identity>` and a fleet snapshot `Signed<Snapshot>`;
`SignedConfiguration` carries the public snapshot as one. The daemon's
`fleet::crypto::open` checks the signature over the carried bytes and only
then parses them, so nothing is re-encoded to be verified, and a payload
that differs by one byte is refused.

### 7.3 Generated schemas (`web/core/src/schemas/`)

The Rust types are the only definition of the client link's types
(ADR 0002, ADR 0003):

1. They derive `schemars::JsonSchema`. `link`'s `schema` example prints
   the document `tools/schemas.sh` writes as
   `crates/link/link.schema.json`, which no Rust code reads: a draft
   2020-12 document with the four client roots, the ticket and every type
   they reach under `$defs`, as serde serializes them, and `link::ALPN` as
   `Alpn`.
2. `web/scripts/schemas.ts` reads that document with Effect's
   `JsonSchema`, `SchemaRepresentation` (objects closed) and `Codegen`, and
   writes `web/core/src/schemas/index.ts`, one Effect Schema per definition.
3. `tools/schemas.sh` runs both and formats their output with oxfmt.
   `tools/schemas.sh --check`, the `schemas` suite, then fails when
   `crates/link/link.schema.json` or `web/core/src/schemas/` differs from what is
   committed or holds an untracked file, and when the document differs
   from main's while `link::ALPN` doesn't. `link` names no path in `web/`;
   the web app pulls the schema.

Nothing there is edited by hand. Encoding is tested only in Rust, by
`link`'s frame tests; the app's tests check what it does with decoded
values, and `web/src/schemas.test.ts` decodes every recorded feed
(§9) with the generated schemas. The schemas carry a ticket only in its
JSON form, so `web/core/src/ticket.ts` is the one reader of a ticket's text
(base64url of its version, host id, token and relay URL), and Rust has
only its writer, `link::Ticket::to_text`. The reader is tested against
the worked example `link` publishes as `crates/link/ticket.example.json`,
whose writer test fails when the file no longer holds what it writes.
`ticket.ts` also holds the app's one `#join=` marker, which the daemon
writes in `start.rs`.

### 7.4 The link (`web/src/link.ts`, `web/src/link/`)

`link(pipe, ticket, client)` starts one host's link in an Effect scope;
the app runs one per paired host, all at once. It reports a stream of
updates (`Status`, `Reset`, `Welcome`, `Feed`) for one reader, takes
commands, and runs until its scope closes, which closes its connection.

- **States.** Each connection walks `Disconnected → Connecting →
  Authenticating → CatchingUp → Live`. A status carries why the last
  connection ended until the next one is live.
- **Dial** (`link/dial.ts`). The link dials through the pipe with the
  ticket's host and relay and the client link's ALPN. A dial that hasn't
  connected in 1.5 s is hedged with a fresh one; the first to connect wins,
  and every other is closed once it settles.
- **Hello.** The first stream sends `Hello { client, token }`, where
  `client` is the device's kind (`web/core/src/device.ts`). A `Welcome` must
  arrive within 10 s; a refusal or a timeout ends the connection.
- **Cursor** (`link/feed.ts`). The link keeps the epoch and last applied
  seq. A welcome from another epoch, or a first one, sets the cursor to the
  epoch's start and emits `Reset` before the `Welcome`, so the reader clears
  the host's history before the replay. The subscription asks for events
  after the cursor's seq. A stored event at or below the cursor is a replay
  and dropped; one past the next is a gap and ends the connection. A live
  delta (`AssistantDelta`, `ReasoningDelta`) repeats the last stored seq and
  is forwarded without moving the cursor. Other feed frames are forwarded
  as they come. The link is `Live` once it has applied the welcome's
  `head_seq`.
- **Reconnect.** When a connection ends, the link reports `Disconnected`
  and dials again after a jittered exponential backoff, from 250 ms
  doubling to 10 s, starting from 250 ms again after a connection that went
  live. `reconnect` drops the connection and dials at once; the page calls
  it when the browser comes back online or the page becomes visible.
- **Commands** (`link/command.ts`). Each command opens its own stream,
  sends `Command { id, body }` with an id from Effect `SecureRandom`, provided by `BrowserCrypto`, finishes the
  stream and returns the one `CommandResult` to its caller. `send` waits
  while the link connects and fails with `NotConnected` while it is down;
  the caller decides whether to retry.

### 7.5 The fold (`web/core/src/fold/`)

Pure functions over the generated types, with Effect's `Data` and `Match`
for tagged values. It is the only fold of a host's events.

- `fold/host.ts`: a host's name, catalog, instances, the home's projects
  and its sessions. A welcome replaces what the host advertises and keeps
  the sessions, which the reconnect catches up; `resetHistory` clears them.
  A feed's events go to their sessions, its `Instances` and `Home` replace
  those; sign-ins and the home's issues and flow are not folded. An event
  for a session never created is dropped, and a removed session is gone.
- `fold/session.ts` and `fold/transcript.ts`: each session's transcript of
  messages, streaming replies and reasoning, tool cards, approvals and
  questions with how they were resolved, its status and where it stands
  with finishing on its issue.
- `fold/subagent.ts`: a session's sub-agents, each under the tool call that
  started it, with its own transcript and requests.
- `fold/offers.ts`: what a session offers, decided here so views only
  render it: whether the composer is open, busy or closed, and whether to
  offer Interrupt, Continue and Finish; its open requests, its own and its
  sub-agents'; and the status the list shows.

## 8. The app (`web/`)

Vite, React and Effect, checked by the standards' `typescript/check.sh`
and tested through `@effect/vitest`. `npm run build` builds the pipe and then the app
into `web/dist`, which Pages serves (`.github/workflows/pages.yml`).

The npm workspace separates three packages (ADR 0004). `web/core` owns
pure folds, routes, ticket decoding, storage decisions, commands and view
models. `web` owns React views and browser effects. `web/scripts` owns the
Node schema generator. Core exports use `@conductor/core/<path>`; UI
imports use `#ui/<path>` and `#transport` names the generated wasm pipe.

- **Store** (`web/src/store.ts`). One store over every paired host: its
  ticket, link status and fold, and the screen shown. Each host's link runs
  in its own fiber of a `FiberMap`, and forgetting a host closes its fiber.
  Updates from a link since replaced or forgotten are dropped. An Effect
  atom holds the view, and an atom registry owns its updates. Lifecycle
  effects run within the store's scope. React reads the view through
  `useAtomValue` in `web/src/use-store.ts`.
  Pure transitions and selectors live in `web/core/src/store.ts`; the
  browser supplies the pipe, storage, fragment and history so tests can
  supply them through `web/src/test/page.ts`. Browser event streams and
  storage use `BrowserStream` and `BrowserKeyValueStore`.
  The store owns pairing, ticket replacement and forgetting through their
  pending, successful and failed outcomes. Operations on one host coordinate
  storage writes and connection ownership; different hosts connect independently.
  Forget closes the connection even when deleting the saved pairing fails.
  A failed Forget keeps a disconnected row with a retry action. Older storage
  writes and restoration reads cannot revive a successfully forgotten host.
- **Pairing.** A host pairs by a join link (`#join=`) opened on the device
  or by a join link or ticket pasted into the pairing screen. A join link's
  fragment is replaced in history after its pairing is saved. A failed
  write keeps the join link available for retry. Pairing a paired host again
  replaces its ticket through the same tracked operation.
- **Storage** (`web/src/saved.ts`). One local storage record per host
  under `conductor.v2.host.<endpoint id in hex>`, holding its ticket and
  the name from its last welcome. A record in any other format, the Dioxus
  app's included, is set aside unread, and those hosts are paired again.
  Restoring a saved pairing starts its connection without rewriting the
  record. Storage failures reach the store as typed outcomes.
- **Routes** (`web/core/src/route.ts`). The screen is the URL fragment, so Pages
  serves one path: `#/pair`, `#/` (the sessions), `#/host/<host>/new` and
  `#/host/<host>/session/<id>`. With no host paired every fragment shows
  pairing, and an unknown one shows the sessions.
- **Screens** (`web/src/app.tsx`, `web/src/session/`):
  - The sidebar: each host under its name and link state, with its last
    error, its sessions most recently active first, each with its open
    requests or its status, and *New session*.
  - Pairing: the join link or ticket field and each paired host with its
    link state and *Forget*.
  - New session: a host, one of its instances, one of its projects or a
    directory, a model and an effort.
  - A session: its live transcript with tool cards and sub-agents under
    their call, a dock of its open approvals and questions with one-tap
    answers, and the composer with *Stop*, *Continue* and *Finish* where
    the fold offers them. Each action sends one command on the host's link.
  - On a phone the sidebar and the pane are a stack: the sessions route
    shows the list, and any other route its pane.
- **Installable.** `web/public/manifest.webmanifest` with the existing
  icons, standalone, installable on the desktop and the iPhone Home
  Screen. There is no service worker.

We port T3 Code's designs, not its code (`apps/web/src/components/`):

| Ours | T3 Code reference | Thin app |
|---|---|---|
| Sidebar: hosts and sessions with status | `AppSidebarLayout.tsx`, `ConnectionStatusDot.tsx` | yes |
| Transcript, markdown, code | `ChatView.tsx`, `ChatMarkdown.tsx` | yes; paragraphs, fenced and inline code |
| Tool-call cards | `CommandBlock.tsx` | yes |
| Approval and question panel | `ComposerContextPermissions.tsx` | yes |
| Diff panel per turn | `DiffPanel.tsx` | later |
| Branch toolbar, command palette | `BranchToolbar*.tsx`, `CommandPalette*.tsx` | later |
| Terminal | `ThreadTerminalDrawer.tsx` | no |

The phone layout follows T3 Code's `docs/internals/mobile-layout.md`.
Turn diffs, the projects screen, issues, credentials and sign-ins, fleet
invites and browser device identity, and Web Push are not in the thin
app; later specs bring them back.

**Web Push.** The host persists its VAPID signing key and browser
subscriptions in private files. It sends encrypted routing metadata (host,
session, event sequence and expiry) when a new approval or question opens,
never the pairing credential, transcript, prompt, question text or tool
arguments. It removes subscriptions that the browser's push service reports
as expired. The app has no service worker, so nothing subscribes yet.

## 9. Testing

- **Replay tests** run with `cargo test -p claude-cli -p codex-cli`. The
  private replays beside the Claude session driver and Codex server
  (`crates/claude-cli/src/session/replay.rs`,
  `crates/codex-cli/src/server/replay.rs`) assert native frames, ordering
  and errors over real pipes. Integration tests in each shell's `tests/`
  directory enter through the plugin and observe shared runtime events.
  Both use the fake CLIs and recorded fixtures.
- **Daemon tests** run in `daemon`'s own modules. Its `fleet::crypto`
  tests refuse a signed payload whose bytes differ by one byte. An
  end-to-end test is added only as the regression test of an observed
  bug.
- **The app's tests** (`@effect/vitest`, in `web/`) replay recorded host feeds
  (`web/fixtures/feeds/`, frozen data in the client link's shape)
  through the fake pipe (`web/src/test/recorded-feeds.ts`,
  `web/src/test/replay.ts`, where a fake host serves a feed's welcome,
  frames and the results and frames that answer each command). They cover
  the link going live from a fresh cursor, resubscribing after a dropped
  connection with no duplicated or missing event, and resetting and
  replaying on an epoch change; the fold's transcripts, tool cards, open
  requests and offers, and a request resolved on another device; pairing,
  forgetting and the storage format; and each session action sending its
  command, with the view following the feed that answers it.
- **Fixtures and run records.** The commit gate checks generated
  recordings against their builders and checks retained run records for
  public IP addresses.
- **Suites** (`checks.toml`, run by `tools/check.sh` beside the Rust
  standard's checks). At `commit`: `ip-scrub`, `tooling`, `e2e-fixtures`,
  `wasm-clippy` (clippy on `web-iroh` for `wasm32-unknown-unknown`),
  `schemas` (§7.3) and `web` (the TypeScript standard's commit checks on
  `web/`, after building it). At `step`: `web-step` (the TypeScript
  standard's step, adding the Vitest run).
- **Manual checks** (`docs/smoke.md`, `docs/manual-acceptance.md`): real
  CLIs, the desktop browser and the iPhone on mobile data.

## 10. References

| Source | License | Use |
|---|---|---|
| pingdotgg/t3code | MIT | Adapter behaviour, receipts, checkpoints, UI designs |
| anthropics/claude-agent-sdk-python | MIT | Claude's control protocol |
| openai/codex (`codex-rs/app-server`, its JSON Schema) | Apache-2.0 | Codex's protocol |
| getpaseo/paseo | Apache-2.0 | Provider option sets, timeline epochs |
| BloopAI/vibe-kanban (`executors`) | Apache-2.0 | Second reference for the clients |
| hardbeat920/monocode | MIT | SSH host bootstrap, provider lifecycle |
| n0-computer/iroh | Apache-2.0/MIT | Transport |
