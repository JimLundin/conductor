# P1 slice 1: the real host (design for review)

This is a design, not an implementation. It replaces the simulated
`conductord s3-host` with a host that runs real Claude Code and Codex
sessions through `crates/agents`. The owner reviews it before any code is
written. Every sketch is written in the subset (`subset.toml` version 5).
Function bodies are `todo!()` here and only here.

## 0. Scope and the shape in one page

**In scope** (the owner's decisions):

- Real sessions. Start a Claude or Codex session in a directory the user
  picks, send messages, stream replies, approve or deny tool calls,
  interrupt, and see turns finish.
- One host with several sessions. One shared `CodexServer` per host, one
  `ClaudeSession` per Claude session.
- An in-memory event log with the spike's gap-free replay (`after_seq`,
  epoch).
- The app gets a session list, a "new session" form (provider plus
  directory), and the existing transcript and approval UI per session.
- The daemon e2e and the browser e2e run the real daemon against fake
  CLIs. A manual smoke test runs the real CLIs.
- The simulated agent is deleted.

**Out of scope:** git worktrees, SQLite durability, the fleet key, pairing
beyond today's ticket, several hosts. Also out, decided here: idempotent
command receipts (the client never retries a command), Codex questions
(`item/tool/requestUserInput`), permission modes, model choice, resume after
a daemon restart.

**The shape.** The protocol becomes per-session: every event names its
session, and every command after `StartSession` names the session it acts on.
The daemon has four kinds of actor: one `Log` (the event writer), one
`Registry` (session ids to handles), one `Session` per session (owns its agent
and its open requests), and one `CodexHub` (owns the shared app-server and
routes its events to threads). Agent events become protocol events through
two pure mapping functions, one `match` per provider, with const tables for
the values. The client folds the event stream into a `HostView` keyed by
session id. `HostLink` does not change shape.

```
 app (Dioxus)          client                     daemon
 ───────────           ──────                     ──────
 SessionList  ◄─┐
 NewSession     ├── HostView ◄── HostLink ══ iroh/WebRTC ══ serve ─┬─ Log (seq, replay, broadcast)
 SessionPane  ◄─┘  (fold by                                        ├─ Registry ─┬─ Session 1 ── ClaudeSession
                    session id)                                    │            ├─ Session 2 ── CodexThread ─┐
                                                                   │            └─ Session 3 ── CodexThread ─┤
                                                                   └─ CodexHub ── CodexServer ◄─────────────┘
```

## 1. The protocol (`crates/proto`)

### 1.1 What stays

`HostSeq`, `Epoch`, `CommandId`, `RequestId`, `Open`, `Hello`, `HelloReply`,
`Welcome`, `Subscribe`, `RtcOffer`, `Signal`, `CandidateType`, the frame codec,
the ticket and the relay helper stay as they are. The connection shape in
architecture §5.1 is unchanged: one stream per command, one long-lived
subscription stream, one stream per WebRTC upgrade.

`ALPN` moves from `conductor/0` to `conductor/1`. The event and command types
change incompatibly, so an old web app must fail at the handshake, not in
the middle of a decode.

### 1.2 New types

```rust
/// Assigned by the host, from 1, in order of creation. Unique within an
/// epoch, which is all an in-memory log needs (architecture §5.2's ULID comes
/// with SQLite).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub u64);

/// A tool call's id as the provider gave it (`toolu_…` or a Codex item id).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolCallId(pub String);

/// A path on the host, as the host's OS spells it. The client never
/// interprets it: a browser on a phone can't know a Windows path's rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostPath(pub String);

/// The name a client gave in `Hello`. Shown as "by" on a resolution and a
/// user message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientName(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Claude,
    Codex,
}
```

`Hello.client` becomes a `ClientName`. `RequestId` stays a `u64` but is now
unique per session, not per host, because every request-related command and
event also names its session.

### 1.3 Commands

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub id: CommandId,
    pub body: CommandBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandBody {
    StartSession { provider: Provider, directory: HostPath },
    SendMessage { session: SessionId, text: String },
    Interrupt { session: SessionId },
    Resolve { session: SessionId, request: RequestId, decision: Decision },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approve,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum CommandResult {
    Accepted,
    /// The answer to `StartSession`. `SessionCreated` follows in the log.
    Started { session: SessionId },
    Rejected(Rejection),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Rejection {
    NoSuchDirectory,
    ProviderUnavailable { reason: String },
    UnknownSession,
    SessionClosed,
    TurnInProgress,
    NoTurnRunning,
    UnknownRequest,
    AlreadyResolved { by: ClientName },
}
```

There is **no `ListSessions` command.** The log is the state: a client that
replays from seq 0 sees every `SessionCreated`, so the session list is a fold
like the transcript. A second way to learn the same thing would break quality
6 ("one way").

`Rejection::label` stays one `match` that maps each variant to its text
(quality 7, second form).

### 1.4 Events

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub seq: HostSeq,
    pub session: SessionId,
    pub kind: EventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    SessionCreated { provider: Provider, directory: HostPath, by: ClientName },
    /// The agent process is up and takes messages.
    SessionReady,
    SessionClosed { reason: CloseReason },
    UserMessage { text: String, by: ClientName },
    AssistantDelta { text: String },
    /// The finished text of one assistant message. It replaces the deltas
    /// since the previous message.
    AssistantMessage { text: String },
    ToolCall { call: ToolCallId, phase: ToolPhase },
    RequestOpened { request: RequestId, tool: String, detail: String },
    RequestResolved { request: RequestId, resolution: Resolution },
    TurnCompleted { outcome: TurnOutcome },
    /// Something went wrong that did not end the session.
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum ToolPhase {
    Started { tool: ToolKind },
    Finished { outcome: ToolOutcome },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolKind {
    Command { command: String },
    FileChange { paths: Vec<String> },
    Other { name: String, detail: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutcome {
    Succeeded,
    Failed,
    Declined,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "resolution", rename_all = "snake_case")]
pub enum Resolution {
    Decided { decision: Decision, by: ClientName },
    /// The agent withdrew the request before anyone answered.
    Withdrawn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Interrupted,
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum CloseReason {
    FailedToStart { message: String },
    Exited { status: String },
}
```

**There is no `SessionStatus` event.** Status is a fold over facts the log
already holds, so the client derives it (§3.1). A status event next to
`SessionReady`, `UserMessage` and `TurnCompleted` would say everything twice,
and the two could disagree.

**Deltas are durable in slice 1.** Architecture §5.2 makes deltas live-only
and sends a `TurnSnapshot` to a client that reconnects mid-turn. With an
in-memory log, keeping deltas in the log is the spike's behaviour and gives
gap-free replay with no second path. Open question 3 asks whether to keep it.

### 1.5 From the spike to slice 1

| Spike (`proto` today) | Slice 1 | |
|---|---|---|
| `Event { seq, kind }` | `Event { seq, session, kind }` | every event names its session |
| `EventKind::TranscriptDelta { text }` | `AssistantDelta { text }` | renamed |
| `EventKind::AssistantMessage { text }` | `AssistantMessage { text }` | kept |
| `EventKind::ApprovalRequested { id, tool, detail }` | `RequestOpened { request, tool, detail }` | renamed |
| `EventKind::ApprovalResolved { id, decision, by }` | `RequestResolved { request, resolution }` | `Resolution` adds `Withdrawn` |
| `EventKind::TurnFinished` | `TurnCompleted { outcome }` | carries the outcome |
| (none) | `SessionCreated`, `SessionReady`, `SessionClosed`, `UserMessage`, `ToolCall`, `Error` | new |
| `CommandBody::Prompt { text }` | `SendMessage { session, text }` | names the session |
| `CommandBody::Resolve { approval, decision }` | `Resolve { session, request, decision }` | names the session |
| (none) | `StartSession`, `Interrupt` | new |
| `CommandResult::{Accepted, Rejected}` | adds `Started { session }` | |
| `Rejection::{TurnInProgress, UnknownApproval, AlreadyResolved}` | `TurnInProgress`, `UnknownRequest`, `AlreadyResolved`, plus five new | |
| `Hello.client: String` | `Hello.client: ClientName` | |
| `ALPN = conductor/0` | `conductor/1` | |

Deleted outright: `TranscriptDelta`, `ApprovalRequested`, `ApprovalResolved`,
`TurnFinished`, `Prompt`, `UnknownApproval`. Nothing is kept for
compatibility. The web app and the daemon ship together.

## 2. The daemon (`crates/daemon`)

### 2.1 Module layout

The crate is replaced, not rewritten. Every file is new subset code. Where
logic carries over (the gap-free subscribe, the WebRTC host), the new file
ports the behaviour, not the text.

```
crates/daemon/src/
  main.rs          clap; `conductord host`; builds Log, CodexHub, Registry, Router
  lib.rs           pub mod list; load_or_create_key; random
  log.rs           Log actor: seq, replay, live broadcast
  registry.rs      Registry actor: SessionId -> Session handle; StartSession
  session.rs       Session actor: one agent, its turn, its open requests
  codex_hub.rs     CodexHub actor: the shared CodexServer and thread routing
  mapping.rs       pure: ClaudeEvent / ThreadEvent -> Vec<Step>; const tables
  serve.rs         ProtocolHandler: Hello, command dispatch, subscribe feed
  wire.rs          SendHalf / RecvHalf over iroh or a data channel
  rtc.rs           the WebRTC host (str0m), ported into the subset
  stun.rs          STUN binding requests, split out of rtc.rs
crates/daemon/tests/
  e2e.rs           real daemon + fake CLIs + HostLink, over loopback iroh
```

Deleted: `host.rs` (the simulated agent and its log), `server.rs` (becomes
`serve.rs`), the `S3Host` subcommand.

### 2.2 Why these actors (design it twice)

Two whole shapes were compared.

**A. One host actor** owns the log, every session's state and the Codex
routes, as the spike's `Host` does. Its surface is the smallest: `serve.rs`
keeps today's three calls. But every agent call runs inside its loop.
`ClaudeSession::interrupt` waits for the CLI's control response for up to
`CONTROL_TIMEOUT` (60 s, `agents/src/claude/session.rs`), and
`ClaudeSession::spawn` waits for `initialize`. One slow call would stall every
session and every subscriber. Taking the calls off the loop needs a "call in
flight" state per session and a reply message per call, which is the
per-session actor written inside out.

**B. Per-session actors (chosen).** Each session's state belongs to one task,
so nothing is shared and nothing is locked
(`principle-separate-before-serializing-shared-state`). A slow agent call
stalls only its own session. The log is its own actor, so subscribing never
waits on an agent. The cost is two more small actors (`Registry`, `CodexHub`)
and one extra hop for commands, which `serve.rs` takes once per stream.

### 2.3 The log

The spike's log, cut out of `host.rs` and minus the simulation.

```rust
/// What a new subscriber needs: the log after its cursor, and the live feed,
/// attached in the same actor turn as the replay, so nothing falls between.
pub struct Subscription {
    pub replay: Vec<Event>,
    pub live: broadcast::Receiver<Event>,
}

#[derive(Debug, Clone)]
pub struct Log {
    inbox: mpsc::Sender<LogMsg>,
}

enum LogMsg {
    Append { session: SessionId, kind: EventKind },
    Welcome { reply: oneshot::Sender<Welcome> },
    Subscribe { after: HostSeq, reply: oneshot::Sender<Subscription> },
}

impl Log {
    pub fn spawn(name: String, epoch: Epoch) -> Log { todo!() }
    /// Fire and forget: the log assigns the seq. Events of one session stay
    /// in order because one task sends them, through one channel.
    pub async fn append(&self, session: SessionId, kind: EventKind) -> Result<(), Stopped> { todo!() }
    pub async fn welcome(&self) -> Result<Welcome, Stopped> { todo!() }
    pub async fn subscribe(&self, after: HostSeq) -> Result<Subscription, Stopped> { todo!() }
    async fn ask<T>(&self, build: impl FnOnce(oneshot::Sender<T>) -> LogMsg) -> Result<T, Stopped> { todo!() }
}

struct LogActor {
    name: String,
    epoch: Epoch,
    events: Vec<Event>,
    live: broadcast::Sender<Event>,
}
```

`serve.rs` runs the subscribe feed: the replay, then the live events, and it
closes the stream on `Lagged` so the client resubscribes from its cursor. The
log takes the replay and attaches the live receiver in one actor turn, so the
two neither overlap nor leave a gap.

### 2.4 The registry

```rust
#[derive(Debug, Clone)]
pub struct Registry {
    inbox: mpsc::Sender<RegistryMsg>,
}

enum RegistryMsg {
    Start { provider: Provider, directory: HostPath, by: ClientName, reply: oneshot::Sender<CommandResult> },
    Find { session: SessionId, reply: oneshot::Sender<Option<Session>> },
}

/// How the host runs each provider. Built once in `main` from the flags.
#[derive(Debug, Clone)]
pub struct Providers {
    pub claude: ClaudeLaunch,
    pub codex: CodexAvailability,
}

#[derive(Debug, Clone)]
pub struct ClaudeLaunch {
    pub claude_path: PathBuf,
}

#[derive(Debug, Clone)]
pub enum CodexAvailability {
    Ready(CodexHub),
    /// `codex app-server` failed to start at boot; the reason is the error.
    Unavailable(String),
}

impl Registry {
    pub fn spawn(log: Log, providers: Providers) -> Registry { todo!() }
    /// Checks the directory, assigns the id, logs `SessionCreated`, spawns
    /// the session actor and answers `Started` at once. The agent starts
    /// inside the session task, so a slow `initialize` never blocks this actor.
    pub async fn start(&self, provider: Provider, directory: HostPath, by: ClientName) -> Result<CommandResult, Stopped> { todo!() }
    pub async fn find(&self, session: SessionId) -> Result<Option<Session>, Stopped> { todo!() }
}

struct RegistryActor {
    log: Log,
    providers: Providers,
    sessions: BTreeMap<SessionId, Session>,
    next: SessionId,
}
```

`StartSession` is checked at this boundary and nowhere else
(`principle-boundary-discipline`): the directory must exist and be a
directory (`Rejection::NoSuchDirectory`), and a Codex session needs
`CodexAvailability::Ready` (`Rejection::ProviderUnavailable`). Claude has no
availability state. A missing `claude` binary shows up as
`SessionClosed { FailedToStart }`, with the spawn error as the message.

### 2.5 The session actor

```rust
/// A handle to one session's task. Clone it freely.
#[derive(Debug, Clone)]
pub struct Session {
    inbox: mpsc::Sender<SessionMsg>,
}

enum SessionMsg {
    Send { text: String, by: ClientName, reply: oneshot::Sender<CommandResult> },
    Interrupt { reply: oneshot::Sender<CommandResult> },
    Resolve { request: RequestId, decision: Decision, by: ClientName, reply: oneshot::Sender<CommandResult> },
}

impl Session {
    pub async fn send(&self, text: String, by: ClientName) -> Result<CommandResult, Stopped> { todo!() }
    pub async fn interrupt(&self) -> Result<CommandResult, Stopped> { todo!() }
    pub async fn resolve(&self, request: RequestId, decision: Decision, by: ClientName) -> Result<CommandResult, Stopped> { todo!() }
    async fn ask<T>(&self, build: impl FnOnce(oneshot::Sender<T>) -> SessionMsg) -> Result<T, Stopped> { todo!() }
}

/// What the session task needs to start its agent.
pub(crate) struct SessionSpec {
    id: SessionId,
    directory: HostPath,
    launch: Launch,
    log: Log,
}

pub(crate) enum Launch {
    Claude(ClaudeLaunch),
    Codex(CodexHub),
}

/// The agent, once started. Both handles are actors in `agents`.
enum Agent {
    Claude(ClaudeSession),
    Codex(CodexThread),
}

/// Where the agent's events arrive. Owned by the loop state, so its
/// `&mut self` method is allowed (0008, version 4).
enum Feed {
    Claude(mpsc::Receiver<ClaudeEvent>),
    Codex(mpsc::Receiver<ThreadEvent>),
}

/// One provider event, before mapping.
enum Native {
    Claude(ClaudeEvent),
    Codex(ThreadEvent),
}

enum Turn {
    Idle,
    /// Codex needs the turn id to interrupt; Claude interrupts the session.
    Running(NativeTurn),
}

enum NativeTurn {
    Claude,
    Codex(TurnId),
}

/// The provider's own id for an open request, which `agents` answers.
#[derive(Clone)]
enum NativeRequest {
    Claude(PermissionRequestId),
    Codex(ApprovalId),
}

/// First decision wins (architecture §8). A resolved request keeps who
/// resolved it, so a late answer is told by whom.
enum Pending {
    Open(NativeRequest),
    Resolved(Resolution),
}

struct SessionActor {
    id: SessionId,
    agent: Agent,
    feed: Feed,
    log: Log,
    turn: Turn,
    requests: HashMap<RequestId, Pending>,
    next_request: RequestId,
}

enum Input {
    Command(SessionMsg),
    Agent(Native),
    /// The agent's event stream ended: the process is gone.
    AgentGone,
    /// Every handle was dropped.
    Closed,
}

/// Starts the agent, logs `SessionReady` or `SessionClosed`, then runs.
pub(crate) async fn run_session(spec: SessionSpec, inbox: mpsc::Receiver<SessionMsg>) { todo!() }

impl Feed {
    async fn next(&mut self) -> Option<Native> {
        match self {
            Feed::Claude(events) => events.recv().await.map(Native::Claude),
            Feed::Codex(events) => events.recv().await.map(Native::Codex),
        }
    }
}

impl SessionActor {
    async fn run(mut self, mut inbox: mpsc::Receiver<SessionMsg>) -> Result<Infallible, Stopped> {
        loop {
            let input = tokio::select! {
                msg = inbox.recv() => msg.map_or(Input::Closed, Input::Command),
                event = self.feed.next() => event.map_or(Input::AgentGone, Input::Agent),
            };
            self.on(input).await?;
        }
    }

    async fn on(&mut self, input: Input) -> Result<(), Stopped> { todo!() }
    /// Applies the steps that `mapping` returns, in order.
    async fn apply(&mut self, steps: Vec<Step>) -> Result<(), Stopped> { todo!() }
}
```

Commands, by turn state. The table is the `match (msg, &self.turn)` in
`SessionActor::on`.

| Command | `Turn::Idle` | `Turn::Running` |
|---|---|---|
| `Send` | log `UserMessage`; `send_user` or `send_turn`; turn becomes `Running`; `Accepted` | `Rejected(TurnInProgress)` |
| `Interrupt` | `Rejected(NoTurnRunning)` | `interrupt` (Claude) or `interrupt(turn)` (Codex); `Accepted`. `TurnCompleted { Interrupted }` follows from the agent |
| `Resolve` | as in the next table | as in the next table |

`Resolve`, by the request's state. This is the spike's first-decision-wins,
moved into the session that owns the request.

| `requests.get(request)` | Result | Effect |
|---|---|---|
| `None` | `Rejected(UnknownRequest)` | none |
| `Some(Open(native))` | `Accepted` | log `RequestResolved { Decided }`; answer the agent (§2.7); store `Resolved` |
| `Some(Resolved(Decided { by, .. }))` | `Rejected(AlreadyResolved { by })` | none |
| `Some(Resolved(Withdrawn))` | `Rejected(UnknownRequest)` | none |

When the agent's stream ends (`Input::AgentGone`), the session logs
`SessionClosed { Exited }`, and the actor ends. Its handle then fails with
`Stopped`, which `serve.rs` reports as `Rejected(SessionClosed)`.

### 2.6 The shared Codex server

```rust
#[derive(Debug, Clone)]
pub struct CodexHub {
    inbox: mpsc::Sender<HubMsg>,
}

enum HubMsg {
    StartThread {
        options: ThreadOptions,
        reply: oneshot::Sender<Result<StartedThread, codex::Error>>,
    },
}

/// A new thread and the channel its events arrive on.
pub struct StartedThread {
    pub thread: CodexThread,
    pub events: mpsc::Receiver<ThreadEvent>,
}

#[derive(Debug, thiserror::Error)]
pub enum HubError {
    #[error(transparent)]
    Codex(#[from] codex::Error),
    #[error(transparent)]
    Stopped(#[from] Stopped),
}

impl CodexHub {
    /// Starts `codex app-server` once, at boot. `Err` becomes
    /// `CodexAvailability::Unavailable`.
    pub async fn spawn(options: ServerOptions) -> Result<CodexHub, codex::Error> { todo!() }
    pub async fn start_thread(&self, options: ThreadOptions) -> Result<StartedThread, HubError> { todo!() }
    async fn ask<T>(&self, build: impl FnOnce(oneshot::Sender<T>) -> HubMsg) -> Result<T, Stopped> { todo!() }
}

struct HubActor {
    server: CodexServer,
    routes: HashMap<ThreadId, mpsc::Sender<ThreadEvent>>,
}

enum HubInput {
    Command(HubMsg),
    Codex(CodexEvent),
    Closed,
}
```

The hub alone reads the server's `CodexEvent` stream and alone owns the
routes. It starts a thread **inside** its loop: it awaits `start_thread`, then
inserts the route, then replies. Events for the new thread wait in the
server's channel until the route exists, so none arrive unrouted, and no
buffer of early events is needed. The await is one local JSON-RPC round trip.

Routing, as the hub's one `match` on `CodexEvent`:

| `CodexEvent` | Hub action |
|---|---|
| `Thread { thread, event }` with a route | send `event` to that session's channel |
| `Thread { thread, event }` without a route | `tracing::debug!` and drop (a thread of a closed session) |
| `Exited { status }` | drop every route, which ends every Codex session's `Feed` (`SessionClosed { Exited }`) |
| `Unknown(_)` | `tracing::debug!` |

`respond` and `interrupt` need no hub: `CodexThread` and `CodexServer` are
handles to the agents actor, which answers each approval exactly once.

Thread options for slice 1 are fixed, following Paseo's preset:
`ApprovalPolicy::OnRequest`, `Sandbox::WorkspaceWrite`, `model: None`,
`effort: None`, `cwd` the session's directory. The server's own `cwd` is the
data directory.

### 2.7 Agent events to protocol events

`mapping.rs` is pure: provider event in, `Vec<Step>` out. It does not know
request ids or turn state; the session actor applies the steps.

```rust
pub(crate) enum Step {
    Emit(EventKind),
    /// The actor assigns the `RequestId`, stores `Pending::Open`, and logs
    /// `RequestOpened`.
    Open { native: NativeRequest, tool: String, detail: String },
    /// The actor finds the id, stores `Resolved(Withdrawn)`, and logs it.
    Withdraw(NativeRequest),
    /// The actor sets `Turn::Idle` and logs `TurnCompleted`.
    TurnEnded(TurnOutcome),
    /// Codex's own turn id, which `send_turn` also returns.
    TurnStarted(NativeTurn), // NativeTurn::Claude | NativeTurn::Codex(TurnId)
}

pub(crate) fn claude_steps(event: ClaudeEvent) -> Vec<Step> { todo!() }
pub(crate) fn codex_steps(event: ThreadEvent) -> Vec<Step> { todo!() }
pub(crate) fn claude_answer(decision: Decision, by: &ClientName) -> PermissionDecision { todo!() }
pub(crate) fn codex_answer(decision: Decision) -> codex::Decision { todo!() }
```

Each function is one `match` from a variant to its data (quality 7, second
form). The two tables below are those matches.

**Claude** (`agents::claude::ClaudeEvent`):

| Claude event | Steps |
|---|---|
| `TurnStarted { .. }` | `TurnStarted(NativeTurn::Claude)`. It fires on every `system/init`, once per turn (drift-check change). `SessionReady` is still logged after `spawn` returns |
| `Message { Assistant, [Text(t)] }`, per block | `Emit(AssistantMessage { t })` |
| `Message { Assistant, [ToolUse { id, name, detail }] }`, per block | `Emit(ToolCall { id, Started { tool_kind(name, detail) } })` |
| `Message { User, [ToolResult { tool_use_id, outcome }] }`, per block | `Emit(ToolCall { tool_use_id, Finished { outcome } })` |
| `Message { _, [Thinking, Other, Text from User] }` | none |
| `StreamDelta { text }` (added in unit 2) | `Emit(AssistantDelta { text })` |
| `PermissionRequested { id, tool_name, detail }` | `Open { Claude(id), tool_name, detail.text() }` |
| `PermissionCancelled { id }` | `Withdraw(Claude(id))` |
| `TurnFinished { outcome: Succeeded, .. }` | `TurnEnded(Completed)` |
| `TurnFinished { outcome: Failed, terminal_reason }` with the reason in `CLAUDE_INTERRUPTED` | `TurnEnded(Interrupted)` |
| `TurnFinished { outcome: Failed, subtype, .. }` otherwise | `TurnEnded(Failed { subtype })` |
| `Exited { .. }` | none (the stream ends next, which closes the session) |
| `Unknown(_)` | none, `tracing::debug!` |

**Codex** (`agents::codex::ThreadEvent`):

| Codex thread event | Steps |
|---|---|
| `Started` | none |
| `TurnStarted { turn }` | `TurnStarted(NativeTurn::Codex(turn))` |
| `AgentDelta { text, .. }` (added in unit 2) | `Emit(AssistantDelta { text })` |
| `AgentMessage { text, .. }` | `Emit(AssistantMessage { text })` |
| `Item { Started, Command { id, command, .. } }` | `Emit(ToolCall { id, Started { Command { command } } })` |
| `Item { Started, FileChange { id, changes, .. } }` | `Emit(ToolCall { id, Started { FileChange { paths } } })` |
| `Item { Completed, item }` | `Emit(ToolCall { id, Finished { codex_outcome(status) } })` |
| `ApprovalRequested { id, kind: Command { command, .. } }` | `Open { Codex(id), "command", command }` |
| `ApprovalRequested { id, kind: FileChange { reason, .. } }` | `Open { Codex(id), "file change", reason }` |
| `ApprovalRequested { id, kind: Permissions { reason, .. } }` | `Open { Codex(id), "permissions", reason }` |
| `ApprovalRequested { kind: UserInput { .. } }` | `Emit(Error { "Codex asked a question. Questions arrive in a later slice; interrupt to continue." })` |
| `ApprovalCancelled { id }` | `Withdraw(Codex(id))` |
| `TurnCompleted { outcome }` | `TurnEnded(outcome)`, variant for variant |
| `Unknown(_)` | none, `tracing::debug!` |

The values that differ only in data are const lists, not arms:

```rust
/// `terminal_reason` values that mean the user interrupted the turn.
const CLAUDE_INTERRUPTED: &[&str] = &["aborted_streaming", "aborted_tools"];

/// Codex's item `status` strings and what they mean for a tool call.
const CODEX_OUTCOMES: &[CodexOutcome] = &[
    CodexOutcome { status: "completed", outcome: ToolOutcome::Succeeded },
    CodexOutcome { status: "failed", outcome: ToolOutcome::Failed },
    CodexOutcome { status: "declined", outcome: ToolOutcome::Declined },
];

struct CodexOutcome {
    status: &'static str,
    outcome: ToolOutcome,
}
```

Decisions, both providers in one table (`claude_answer`, `codex_answer`):

| `Decision` | Claude `PermissionDecision` | Codex `Decision` |
|---|---|---|
| `Approve` | `Allow` | `Accept` |
| `Deny` | `Deny { message: "Denied by {by}", then: AfterDeny::Continue }` | `Decline` |

**Tool details are parsed in `agents`, not here.** Claude's `ToolUse.input`
and `PermissionRequested.input` are `serde_json::Value`, which is untyped data
and costs 3.0 outside a provider protocol module. Unit 2 adds one typed field
at the boundary, in `agents/src/claude/protocol.rs`, where wire data costs
the boundary weight:

```rust
/// What a tool call is about, read from the CLI's `input` by one const list.
pub enum ToolDetail {
    Command(String),
    Path(String),
    Text(String),
}

/// One row per tool whose input has a field worth showing.
const TOOL_FIELDS: &[ToolField] = &[
    ToolField { tool: "Bash", field: "command", shape: Shape::Command },
    ToolField { tool: "Edit", field: "file_path", shape: Shape::Path },
    ToolField { tool: "Write", field: "file_path", shape: Shape::Path },
    ToolField { tool: "MultiEdit", field: "file_path", shape: Shape::Path },
    ToolField { tool: "Read", field: "file_path", shape: Shape::Path },
    ToolField { tool: "Grep", field: "pattern", shape: Shape::Text },
    ToolField { tool: "Glob", field: "pattern", shape: Shape::Text },
    ToolField { tool: "WebFetch", field: "url", shape: Shape::Text },
];
```

A tool missing from the list gets `Text` with the compact JSON. The daemon's
`tool_kind` maps `Command` to `ToolKind::Command`, `Path` to
`ToolKind::FileChange` for the edit tools and `Other` for the rest.

### 2.8 Serving

`serve.rs` keeps `server.rs`'s structure and ports it into the subset:
authenticate the first stream, then one task per stream. It holds `Log`,
`Registry`, the token and the WebRTC options. The command dispatch is one
`match` on `CommandBody`:

| `CommandBody` | Call |
|---|---|
| `StartSession { provider, directory }` | `registry.start(provider, directory, client)` |
| `SendMessage { session, text }` | `registry.find(session)` then `session.send(text, client)` |
| `Interrupt { session }` | `registry.find(session)` then `session.interrupt()` |
| `Resolve { session, request, decision }` | `registry.find(session)` then `session.resolve(request, decision, client)` |

`find` returning `None` answers `Rejected(UnknownSession)`. A session
handle's `Stopped` answers `Rejected(SessionClosed)`.

### 2.9 The binary

```
conductord host [--data-dir DIR] [--name NAME] [--web-url URL]
                [--claude-path PATH (default "claude")] [--codex-path PATH (default "codex")]
                [--stun URL]... [--no-webrtc] [--test-rtc-blackhole] [--test-rtc-drop-after N]
```

`s3-host` and `--delta-ms` go. The ticket, join link and QR code printing
stay as they are. On Ctrl-C the host stops the router, then calls
`CodexServer::shutdown`; Claude children die with their `ClaudeSession`
handles. Restart behaviour is out of scope: the log, the sessions and the
children all end with the process, and the next start has a new epoch, so
clients start over from seq 0 (the spike's existing path).

## 3. The client and the app

### 3.1 `client`

`HostLink` keeps its states, cursor, backoff and WebRTC upgrade. Only the
`CommandBody` it carries changes type, so `link.rs`, `wire.rs`, `upgrade.rs`
and `platform/` change by imports only. `Update::Reset(epoch)` keeps its
meaning: drop the whole `HostView`.

`transcript.rs` becomes `host_view.rs`: the host's state folded from events,
keyed by session.

```rust
pub struct HostView {
    pub sessions: BTreeMap<SessionId, SessionView>,
}

pub struct SessionView {
    pub provider: Provider,
    pub directory: HostPath,
    pub status: SessionStatus,
    pub items: Vec<Item>,
}

/// Derived, never sent (§1.4).
pub enum SessionStatus {
    Starting,
    Idle,
    Running,
    Closed(CloseReason),
}

pub enum Item {
    User { text: String, by: ClientName },
    Reply { text: String, state: ReplyState },
    Tool { call: ToolCallId, tool: ToolKind, state: ToolState },
    Request { request: RequestId, tool: String, detail: String, state: RequestState },
    TurnCompleted(TurnOutcome),
    Error(String),
}

pub enum ReplyState { Streaming, Done }
pub enum ToolState { Running, Finished(ToolOutcome) }
pub enum RequestState { Open, Resolved(Resolution) }

/// An in-place change to an earlier item. Resolving a request and finishing
/// a tool call are the same shape, so they are one `map` over the items.
enum Change {
    Resolve(RequestId, Resolution),
    Finish(ToolCallId, ToolOutcome),
}

impl HostView {
    pub fn empty() -> HostView { todo!() }
    pub fn apply(self, event: Event) -> HostView { todo!() }
}

impl SessionView {
    /// The decisions the UI renders, made here so markup only shows them
    /// (proposal 0018).
    pub fn controls(&self) -> Controls { todo!() }
    pub fn attention(&self) -> Attention { todo!() }
    fn apply(self, kind: EventKind) -> SessionView { todo!() }
}

pub struct Controls {
    pub composer: Composer,
    pub interrupt: Interrupt,
}

pub enum Composer { Open, Busy, Closed }
pub enum Interrupt { Offered, Hidden }
pub enum Attention { Quiet, Requests(usize) }
```

Status as a fold: `SessionCreated` gives `Starting`, `SessionReady` gives
`Idle`, `UserMessage` gives `Running`, `TurnCompleted` gives `Idle`, and
`SessionClosed` gives `Closed`. The items keep the spike's reply rule: deltas
join into one streaming reply, and `AssistantMessage` replaces that reply's
text and closes it. That last part fixes a real ambiguity: with deltas
durable, a replay would otherwise show the text twice.

The provider choices for the form are one const list:

```rust
pub struct ProviderChoice {
    pub provider: Provider,
    pub label: &'static str,
}

pub const PROVIDERS: &[ProviderChoice] = &[
    ProviderChoice { provider: Provider::Claude, label: "Claude Code" },
    ProviderChoice { provider: Provider::Codex, label: "Codex" },
];
```

### 3.2 `app`

Screens. The selection is UI state, so it lives in the app as one signal:

```rust
enum Screen {
    Sessions,
    NewSession,
    Session(SessionId),
}
```

On a wide window the session list is a sidebar and the selected screen fills
the rest. On a phone (CSS, under 700 px) one screen shows at a time with a
back button, per T3 Code's `mobile-navigation.md`.

| Component | New or changed | What it renders |
|---|---|---|
| `StatusLine` | kept | the link state, path and host name |
| `SessionList` | new | one row per session: provider, directory's last segment, status dot, a request count from `attention()`, and a "New session" button |
| `NewSession` | new | provider radio from `PROVIDERS`, a directory text field, Start. On `Started { session }` it selects that session. A `Rejected` shows `label()` under the form |
| `SessionPane` | new | a header (provider, full directory, status, Interrupt while `Interrupt::Offered`), the transcript, the composer |
| `TranscriptItem` | changed | adds `User`, `Tool`, `Error` and the turn outcome; `Request` is today's approval card, with "Withdrawn" as a third resolved state |
| `Composer` | changed | enabled by `Composer::Open`; sends `SendMessage` for the selected session |

`Pair` and `ticket.rs` stay. `main.rs` splits into `main.rs` (App, Pair,
StatusLine), `sessions.rs` (SessionList, NewSession) and `session.rs`
(SessionPane, TranscriptItem, Composer), so no file grows past today's size.
The CSS gains the list, the form, tool rows and the phone layout.

The browser e2e reads the DOM, so the class names are part of the design:
`.session-row[data-status]`, `form.new-session`, `.session-pane`,
`.user`, `.reply`, `.reply.streaming`, `.tool[data-state]`,
`.request[data-open]`, `.request .resolved`, `.turn-end[data-outcome]`,
`button.interrupt`.

## 4. Tests

### 4.1 The fake CLIs

Today `tests/fake-claude.sh` prints a whole recording at once, and
`codex_replay.rs` writes a Python stand-in that holds each response until its
request arrives. A daemon e2e needs both fakes to wait for the daemon: a turn
must not start before the user's message, and the rest of a turn must not
play before the approval answer.

Unit 1 commits one stand-in, `crates/agents/tests/fake-cli.py`, used by both
`fake-claude.sh` and `fake-codex.sh`. It plays a recording and gates lines on
what it reads from stdin, with two rules:

1. **Implicit (today's Codex rule):** a JSON-RPC response waits for our request
   with the same id; a `serverRequest/resolved` waits for our answer.
2. **Explicit markers in the recording**, so the gates are data, not code:
   - `{"fake": "await", "match": {"type": "user"}}` waits for the next stdin
     line whose JSON has those key/values (a Claude user message, a
     `control_response`, a Codex `turn/start`).
   - `{"fake": "pace", "ms": 30}` sets a delay between the lines that follow,
     so deltas stream visibly in the browser e2e.

Markers are never printed. The existing replay recordings have no markers and
play as today, so `replay.rs` and `codex_replay.rs` keep passing unchanged.

Each fake reads `stdout.jsonl` from its working directory, as `fake-claude.sh`
does now. For Claude that is the session's directory, so a test picks a
Claude script by the directory it starts the session in. For Codex it is the
app-server's working directory, the host's data directory.

E2e recordings live in `fixtures/e2e/`, made from the existing recordings
plus markers:

| Recording | From | Plays |
|---|---|---|
| `claude-approve.jsonl` | `fixtures/claude/session1-…` turn 1 | message, deltas, one `Write` permission, tool result, final text, `result` |
| `claude-deny-interrupt.jsonl` | unit 2's new `s1` mid-stream interrupt step (interrupt on the first `StreamDelta`) | a denied permission, then a turn with deltas that ends `aborted_streaming` |
| `codex-two-threads.jsonl` | `fixtures/codex/server1.jsonl` | two threads: command approval accepted on one, declined on the other, deltas on both |
| `codex-interrupt.jsonl` | `server1.jsonl` | a turn interrupted mid-stream |

Claude deltas are accepted (owner decision 2), so both `claude-*`
recordings come from unit 2's new `s1` recording with
`--include-partial-messages`. Who builds each recording:

| Recording | Built in | Status |
|---|---|---|
| `claude-approve.jsonl` | unit 1, rebuilt in unit 2 | built by `fixtures/e2e/build-claude.py` |
| `claude-deny-interrupt.jsonl` | unit 2 | built by `fixtures/e2e/build-claude.py` from steps 2 and 3c; a `pong` turn follows the interrupt |
| `codex-two-threads.jsonl` | unit 5 | |
| `codex-interrupt.jsonl` | unit 5 | |

Advisories from the unit 1 review (`docs/subset/reviews/p1-u1.md`):
- `fake-cli.py` plays `serverRequest/resolved` after a 5 s fallback even with
  no answer. Unit 5 or 7 replaces it with an explicit `await` marker, so a
  slow click in the browser e2e can't reorder events.
- `await`'s `match` sees top-level keys only. Unit 5 extends it to nested
  keys if two Codex threads' `turn/start` must be told apart.

### 4.2 Daemon e2e (`crates/daemon/tests/e2e.rs`)

In process, as today: a real `Log`, `Registry`, `CodexHub` and `serve`
behind an iroh `Router` on loopback with relays off, the fakes as the CLI
paths, and a real `HostLink` as the client. Each test folds what the client
receives with `HostView::apply` and compares against a literal expected
`SessionView` (`principle-test-behavior-not-implementation`).

| Test | Asserts | Today's test it replaces |
|---|---|---|
| `a_claude_turn_streams_asks_and_finishes` | items equal the literal list: `User`, `Reply` (deltas joined, then final text), `Tool Running`, `Request` resolved by `test-client`, `Tool Finished(Succeeded)`, `TurnCompleted(Completed)`; status `Idle` | `a_turn_streams_asks_for_approval_and_finishes` |
| `a_denied_request_continues_and_an_interrupt_ends_the_turn` | `Request` resolved `Deny`, then `TurnCompleted(Interrupted)` after `Interrupt`; `Interrupt` on an idle session is `Rejected(NoTurnRunning)` | new |
| `two_codex_sessions_share_one_server` | two sessions, one approves and one declines, each session's items hold only its own thread's events | new |
| `first_decision_wins_across_two_clients` | second `Resolve` is `Rejected(AlreadyResolved { by: "desk" })`; only one `RequestResolved` in the log | `host.rs` unit test of the same name |
| `a_second_message_during_a_turn_is_rejected` | `Rejected(TurnInProgress)` | `host.rs` unit test |
| `start_is_checked_at_the_boundary` | 4 rows: missing directory, a file as directory, Codex unavailable, unknown session id; each a literal `Rejection` | new |
| `a_missing_claude_binary_closes_the_session` | `SessionClosed { FailedToStart }` with the spawn error | new |
| `reconnect_mid_turn_resumes_after_the_cursor_without_gaps_or_duplicates` | seqs contiguous across the reconnect; the folded view equals a fresh client's | kept, on a Claude turn |
| `a_new_client_resumes_from_a_dead_clients_cursor` | | kept |
| `a_cursor_from_another_epoch_starts_over_from_the_beginning` | | kept |
| `a_wrong_token_is_refused_and_never_goes_live` | | kept |

Pure unit tests:

- `mapping.rs`: one table test per provider (well over four cases each, so a
  table per quality 7), event in, literal `Vec<Step>` out.
- `host_view.rs`: the fold over a literal event list, including replayed
  deltas followed by the final message, a withdrawn request and a closed
  session.
- `log.rs`: the spike's replay-after-cursor test, moved.

### 4.3 Browser e2e

`tools/s3-browser.sh` becomes `tools/e2e-browser.sh`. It starts
`conductord host --claude-path crates/agents/tests/fake-claude.sh` in a
scratch directory holding `claude-approve.jsonl` as `stdout.jsonl`, with
pacing markers so deltas stream. `browser.mjs` keeps its three scenarios and
its pairing check. The turn helper becomes: open the "New session" form,
choose Claude Code, type the scratch directory, Start, send a message, see
at least three distinct partial reply texts, approve, and wait for
`.turn-end[data-outcome="completed"]`. The expected texts are the
recording's literal text, not the simulation's "42". The `upgrade`,
`fallback` and `drop` checks on the path and the host log stay.

### 4.4 Manual smoke test

`docs/p1/smoke.md` is a checklist the owner runs with real CLIs, about
fifteen minutes and a few cents of API use:

1. `conductord host` on WSL; the desktop app pairs.
2. Claude in a scratch git repo: ask it to create a file, approve; ask for a
   second, deny; start a long answer and interrupt it.
3. Codex in the same repo: the same three steps.
4. Both sessions running at once; the list shows both, with request badges.
5. Reload the web app mid-turn: the transcript replays with no duplicate text.
6. A bad directory and a host without `codex` on `PATH`: the form shows the
   rejection.

## 5. Order

Each unit is one pair-loop task (`.claude/skills/pair/SKILL.md`), ends in one
green `tools/check.sh` commit, and leaves the system working
(`principle-sequence-verifiable-units`).

| # | Unit | Depends on | Parallel with | Done when |
|---|---|---|---|---|
| 1 | **Gated fake CLI.** `fake-cli.py`, the two `.sh` wrappers, `fixtures/e2e/claude-approve.jsonl` (provisional). A small agents test drives `ClaudeSession` through `claude-approve.jsonl`: no event before `send_user`, nothing after the permission before `respond` | none | 2, 3, T | the new test and the existing replay tests pass |
| 2 | **Agents boundary.** Codex `item/agentMessage/delta` as `ThreadEvent::AgentDelta`; Claude `ToolDetail` (§2.7); Claude `--include-partial-messages` and `StreamDelta`, with a new `s1` recording, from which it rebuilds `claude-approve.jsonl` and builds `claude-deny-interrupt.jsonl` | none | 1, 3, T | replay tests assert the deltas and details as literal lists; `s1` and `s2` pass live, including the new mid-stream interrupt step (interrupt on the first delta) and the immediate interrupt step (before `system/init`), 5 runs in a row; it also carries the drift-check fixes (`docs/subset/reviews/drift-checks.md`) |
| 3 | **Protocol swap.** New `proto` types and `ALPN`; client `host_view.rs` with its fold tests; daemon `log.rs`, `registry.rs` (both providers `Unavailable`, the permanent path for a missing CLI), `serve.rs`, `main.rs` as `host`; `host.rs` and the spike types deleted; the app compiles against `HostView` and shows each session's transcript in order (the screens are unit 6) | T | 1, 2 | e2e: the wrong-token and epoch tests pass, and `StartSession` answers `ProviderUnavailable`; `log.rs` unit tests cover replay after the cursor; the app builds for web and desktop |
| 4 | **Claude sessions.** `session.rs`, `mapping.rs` (Claude half), Claude in the registry | 1, 2, 3 | 6, T | the Claude e2e tests in §4.2 pass |
| 5 | **Codex sessions.** `codex_hub.rs`, `mapping.rs` (Codex half), Codex in the registry; builds `codex-two-threads.jsonl` and `codex-interrupt.jsonl` | 4 | 6, T | the Codex e2e tests pass, including two sessions on one server |
| 6 | **App screens.** `SessionList`, `NewSession`, `SessionPane`, the new items, phone layout, and `ticket.rs` into the subset | 3 | 4, 5, T | `dx serve` against a unit-4 host runs a Claude turn by hand; screenshots at desktop and 390x844 |
| T | **Transport port.** `wire.rs`, `rtc.rs`, `stun.rs` into the subset, same behaviour, against today's simulated host (open question 1) | none | 1, 2, and 3's development; lands before 3 | `subset check` is clean on those files; `tools/s3-browser.sh` passes all three scenarios |
| 7 | **Browser e2e.** `tools/e2e-browser.sh`, `browser.mjs` on the real daemon and the fakes | 4, 6, T | 5 | all three scenarios pass in Chromium and WebKit |
| 8 | **Smoke and docs.** Run `docs/p1/smoke.md`; update architecture §12 and the subset report | 5, 7 | none | the owner has run the checklist; `SUBSET=1 tools/check.sh` reports nothing under `crates/daemon` |

Units 1, 2, 3 and T start at once; T lands first, so its proof can still use
the simulated host before unit 3 deletes it. Unit 3 is the widest single
commit, because the protocol change touches every crate. It stays reviewable
because it adds no agent code. Unit 4 is where the first real session runs.

The browser e2e is not part of `tools/check.sh`. It cannot run a turn from
unit 3 until unit 7, because the simulated agent is gone and the screens are
not built yet. Unit 7 restores it; no unit depends on it in between.

## 6. Volume

Measured with `cargo build -p subset` and `./target/debug/subset measure` /
`score` on `43d047b`. Additions are estimates, marked so, scaled from the
client's rewrite into the subset, which grew `link.rs`, `platform/rtc.rs` and
`transcript.rs` from 7,458 to 9,946 tokens (+33%, measured at `b4181b9^`
against today).

**Deleted or replaced (measured):**

| Code | Tokens | Lines | Violations |
|---|---|---|---|
| `daemon/src/host.rs` (log 1,214, simulation 211, tests 869) | 2,294 | 352 | 38 |
| `daemon/src/server.rs` | 1,514 | 214 | 33 |
| `daemon/src/rtc.rs` | 5,002 | 725 | 182 |
| `daemon/src/wire.rs` | 510 | 67 | 6 |
| `daemon/src/lib.rs` | 556 | 62 | 11 |
| `daemon/src/main.rs` | 964 | 157 | 14 |
| `daemon/tests/e2e.rs` | 1,997 | 281 | 32 |
| **daemon total** | **12,837** | **1,858** | **316** |
| `proto` spike event and command types | 345 | 70 | 0 |
| `client/src/transcript.rs` | 938 | 176 | 0 |
| `app/src/main.rs` (rewritten and split) | 1,754 | 255 | 11 |
| **Total** | **15,874** | **2,359** | **327** |

**Added (estimated):**

| Code | Tokens | Lines |
|---|---|---|
| `proto` new types | 1,050 | 220 |
| `daemon` `log`, `registry`, `session`, `codex_hub` | 3,400 | 520 |
| `daemon` `mapping` with its tables | 1,350 | 210 |
| `daemon` `serve`, `lib`, `main` | 3,200 | 480 |
| `daemon` `wire`, `rtc`, `stun` (unit T) | 6,900 | 980 |
| `daemon` unit tests and `tests/e2e.rs` | 4,100 | 590 |
| `client/src/host_view.rs` with tests | 1,700 | 280 |
| `app` (three files) | 3,200 | 430 |
| `agents` deltas and `ToolDetail`, with tests | 900 | 140 |
| **Total Rust** | **25,800** | **3,850** |
| Not Rust: `fake-cli.py`, wrappers, `fixtures/e2e/`, `browser.mjs`, `e2e-browser.sh`, CSS, `smoke.md` | | about 450 |

**Net:** about +9,900 tokens and +1,500 lines of Rust (estimated). Of that,
unit T is about +1,400 tokens of pure subset inflation, which is the price of
open question 1. The rest is new behaviour: real agents, several sessions,
tool calls, and the screens for them. Every daemon violation is gone at the
end. `app/src/ticket.rs`'s 8 violations are assigned to unit 6, which rewrites the app's screens: after unit 6, `crates/` is fully in the subset.

## 7. Open questions

1. **Port the WebRTC host (unit T) in slice 1, or carry `rtc.rs` and
   `wire.rs` over unchanged?** Carrying them saves about 1,400 tokens of
   inflation and one parallel track, but leaves 188 violations in a crate the
   brief says is replaced, and blocks making `SUBSET=1` the gate.
   **Recommendation: port it now,** as the parallel track it is. It touches no
   file the other units touch.
2. **Claude deltas in slice 1?** Codex already sends deltas
   (`item/agentMessage/delta`, 58 in `server1.jsonl`). Claude only does with
   `--include-partial-messages`, which needs a parser change and one new live
   `s1` recording, a few cents of API use. Without it, a Claude reply appears
   all at once when its message ends. **Recommendation: yes.** Streaming is
   most of what makes the transcript feel alive (`principle-experience-first`).
3. **Keep deltas in the log?** Slice 1 logs them as durable events, as the
   spike does, so replay has one path. Architecture §5.2 makes them
   live-only plus a `TurnSnapshot`. **Recommendation: keep them durable for
   slice 1,** and make them live-only in the SQLite slice, where log size
   starts to matter.
4. **How does the user pick a directory?** Slice 1 has a text field that the
   host checks. A host-side browser needs a `ListDirectory` command and a
   picker. **Recommendation: the text field,** with the directories of
   earlier sessions (already in the `HostView`) offered as suggestions.
   Worktrees will change this screen anyway.

## 8. Owner decisions (2026-11-11)

All four open questions are answered as recommended:
1. Unit T ports the WebRTC host into the subset in slice 1.
2. Claude replies stream in slice 1 (`--include-partial-messages`).
3. Deltas stay in the in-memory log for this slice; they become live-only when SQLite arrives.
4. The directory is a text field the host validates, with earlier sessions' directories offered as suggestions.

Also approved: the hedged dial from the reconnect forensics (`docs/forensics/reconnect/` on the local branch `reconnect-forensics`), as its own `client` unit. Git worktrees stay out of slice 1. The volume estimate (~25,800 added tokens) is treated as P1's budget.

## 9. Changes from the drift-check review (2026-11-11)

- Claude's adapter now emits `TurnStarted` on every `system/init` (§2.7),
  and the live step 3 interrupts at turn start. Nothing interrupted
  mid-stream any more, so unit 2 adds a mid-stream step (interrupt on the
  first delta) to `s1` and `s2`, and `claude-deny-interrupt.jsonl` is built
  from it (§4.1, §5).
- Interrupting before `system/init` was unobserved, and P1 marks a turn
  running on `Send` (§2.5). Unit 2 adds an immediate-interrupt step and
  records the behaviour in `claude/PROTOCOL.md`.
- `codex-interrupt.jsonl` stays mid-stream because it is built from
  `fixtures/codex/server1.jsonl` (unit 5).

## 10. Owner decisions on storage and wire format (2026-11-11)

- **Wire format: `postcard`, not JSON.** Frames keep their length prefix; the
  body is postcard-encoded `proto` types. postcard is not self-describing, so a
  breaking `proto` change bumps the ALPN (`conductor/1` → `/2`). The event log
  stores the same encoding, and `conductord dump` prints it as text. The CLI
  adapters stay JSON: that is the CLIs' own protocol (boundary).
- **Storage: `redb`** (decided 2026-11-11). The owner's rule: durable data
  stays Rust types, with no SQL and no text formats. `redb` is a pure-Rust,
  embedded, ACID, crash-safe key-value store with a stable file format, and
  typed tables (`TableDefinition<K, V>`) that are read and written only
  through Rust calls. One `store` module owns it. Values are the `proto`
  types encoded with `postcard` (the wire encoding), keys are our typed ids,
  and extra lookups are extra typed tables. `conductord dump` prints the store
  as text. Turso was considered and rejected because it is queried in SQL
  text. The browser's offline cache, a later item, uses the browser's own
  storage with the same postcard bytes. Durability is a later slice, not
  slice 1.
- **Storage models: serde-derived types plus typed table consts** (option C,
  decided 2026-11-12). A stored value is a plain struct or enum deriving
  `Serialize` and `Deserialize`, encoded with postcard. Each table is one
  `const` of type `redb::TableDefinition<K, V>`, with `K` and `V` our own
  types, declared in the `store` module. No hand-written codecs and no derive
  macro of our own.

## 11. Changes from the unit 2 review (2026-11-11)

- **Interrupts are held until the turn starts** (§2.5). Sent before the
  CLI's turn-started event, Claude acknowledges and then runs the whole turn
  (10 of 10 logs) and Codex refuses with "no active turn" (10 of 10). Sent
  on the turn-started event, both work every time. The session's turn is a
  state machine:
  `Turn = Idle | Starting(Held) | Running(NativeTurn)`, `Held = Nothing | Interrupt`.
  `Send` moves to `Starting(Nothing)`. `Interrupt` while `Starting` is
  `Accepted` and stored as `Starting(Interrupt)`, with nothing sent yet.
  `Step::TurnStarted(native)` moves to `Running(native)` and sends a held
  interrupt then. Turn end returns to `Idle` and drops a held interrupt.
  Units 4 and 5 each add a gated test that interrupts before the turn starts.
- **`Step::TurnStarted` carries `NativeTurn`** (§2.7): `Claude`, or
  `Codex(TurnId)` because Codex interrupts by turn id.
- **The §2.7 lookup tables are `match`es**, not `const` lists:
  `tool_detail` (done in unit 2) and `CODEX_OUTCOMES` (unit 5). A string-keyed
  table measured about +100% against the `match`.
- **`s2` step 1 runs under `ApprovalPolicy::Untrusted` with a writable
  workspace**, so Codex itself asks before every command and the step no
  longer depends on the model.

## 12. Owner decisions from the unit 4 and unit 6 reviews (2026-11-12)

- **Provider availability travels in `Welcome`.** `Providers { claude, codex }`
  and `Availability` move to `proto`; the host fills them from its registry,
  and the New Session screen shows them before the user tries. ALPN bump.
- **`RequestOpened` is typed and carries the tool call id**:
  `RequestOpened { request, call: ToolCallId, tool: ToolKind }`, so the app
  joins an approval to its tool card. Same ALPN bump as `Welcome`
  (`conductor/2`). Two follow-up decisions, approved while unit 6 built it:
  - **No separate `details` field.** `ToolKind` already is the per-kind typed
    payload, made by the same `tool_kind` mapping as `ToolCall`. When the app
    needs a Write's content or an Edit's diff, it goes into `ToolKind`'s
    variants, so `ToolCall` carries it too.
  - **`call` is required.** `agents` refuses a `can_use_tool` without a
    `tool_use_id` with the control error it sends for an unknown subtype.
    The tool is then refused, so `s1`'s approve step fails if the CLI ever
    drops the id (`claude/PROTOCOL.md`).
- **The client ends leftover state when the host says the turn or the CLI
  ended.** On `TurnCompleted` or `SessionClosed`, the `HostView` fold turns a
  `Running` tool into `Stopped` and an `Open` request into
  `Resolved(Withdrawn)`. Display only: a client disconnecting never stops or
  closes a session; the host keeps running it and a reconnecting client
  replays the log.
- **Proposal 0040 accepted, amended**: the `Some(x).filter(..)` lift stays when
  the predicate combines library booleans.
- **Proposal 0041 accepted, amended**: `for` is the form for a loop whose body
  only performs awaited side effects; every other loop stays adapters.
- **The fixture is gone from `crates/`**; unit 7 regenerates the screenshots
  from the real host running the fake CLIs.
