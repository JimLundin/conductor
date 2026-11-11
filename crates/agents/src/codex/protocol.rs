//! The stdio boundary. Each app-server stdout line is parsed with `codex-codes`
//! and mapped into our own types here, and every line we write is built here.
//! Nothing from `codex-codes` leaves this module.

use std::path::PathBuf;

use codex_codes::protocol::methods;
use codex_codes::{
    AskForApproval, ClientInfo, CommandExecutionRequestApprovalResponse,
    FileChangeRequestApprovalResponse, InitializeCapabilities, InitializeParams, JsonRpcMessage,
    ModelListParams, Notification, PatchChangeKind, ReasoningEffort, RequestId, SandboxMode,
    ServerRequest, ThreadItemPayload, ThreadResumeParams, ThreadStartParams, TurnInterruptParams,
    TurnStartParams, TurnStatus, TurnSteerParams, UserInput,
};
use serde::Serialize;
use serde_json::Value;

/// A Codex thread id, as returned by `thread/start` and accepted by `thread/resume`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThreadId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TurnId(pub String);

/// The JSON-RPC id of a server-to-client request we have to answer.
///
/// Codex numbers its own requests, so the id is unique per app-server process,
/// not per thread.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ApprovalId(pub String);

impl ApprovalId {
    pub(crate) fn of(id: &RequestId) -> ApprovalId {
        ApprovalId(id.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalPolicy {
    Untrusted,
    OnRequest,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sandbox {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

/// Settings for `thread/start` and `thread/resume`.
#[derive(Debug, Clone)]
pub struct ThreadOptions {
    pub cwd: PathBuf,
    pub model: Option<String>,
    /// Passed as the `model_reasoning_effort` config override, such as `low`.
    pub effort: Option<String>,
    pub approval_policy: ApprovalPolicy,
    pub sandbox: Sandbox,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    Interrupted,
    Failed { message: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemPhase {
    Started,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Add,
    Delete,
    Update,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub kind: ChangeKind,
}

/// The items the daemon shows besides agent messages. `status` is Codex's own
/// string (`inProgress`, `completed`, `failed`, `declined`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Command {
        id: String,
        command: String,
        status: String,
        exit_code: Option<i64>,
    },
    FileChange {
        id: String,
        changes: Vec<FileChange>,
        status: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ApprovalKind {
    Command {
        command: Option<String>,
        cwd: Option<String>,
        reason: Option<String>,
    },
    FileChange {
        reason: Option<String>,
        grant_root: Option<String>,
    },
    Permissions {
        reason: Option<String>,
        requested: Value,
    },
    /// `item/tool/requestUserInput`; answer it with [`Decision::Raw`].
    UserInput { questions: Value },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ThreadEvent {
    Started,
    TurnStarted {
        turn: TurnId,
    },
    TurnCompleted {
        turn: TurnId,
        outcome: TurnOutcome,
    },
    /// A finished agent message, not a delta.
    AgentMessage {
        turn: TurnId,
        text: String,
    },
    Item {
        turn: TurnId,
        phase: ItemPhase,
        item: Item,
    },
    ApprovalRequested {
        id: ApprovalId,
        turn: TurnId,
        kind: ApprovalKind,
        /// Codex's own decision values, verbatim. For a command this is
        /// `availableDecisions`; without it, and for a file change, it is the
        /// four plain decisions the schema allows. Empty for permissions and
        /// user input, whose answers are not a decision.
        options: Vec<Value>,
    },
    /// Codex resolved a request we had not answered, so answering it now is an error.
    ApprovalCancelled {
        id: ApprovalId,
    },
    Unknown(Value),
}

#[derive(Debug, Clone, PartialEq)]
pub enum CodexEvent {
    Thread {
        thread: ThreadId,
        event: ThreadEvent,
    },
    Exited {
        status: std::process::ExitStatus,
    },
    /// A frame that names no thread.
    Unknown(Value),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Accept,
    AcceptForSession,
    /// The agent continues the turn without the action.
    Decline,
    /// Declines and also ends the turn.
    Cancel,
    /// Sent verbatim as the JSON-RPC `result`, for a native option or a
    /// user-input answer.
    Raw(Value),
}

/// What an answer to an open request has to look like.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AnswerShape {
    Command,
    FileChange,
    /// Holds the requested profile, which an accept grants back.
    Permissions(Value),
    UserInput,
}

/// What one stdout line means to the server actor.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Inbound {
    Event(CodexEvent),
    /// Answer to a request we sent.
    Response {
        id: RequestId,
        outcome: Result<Value, String>,
    },
    Approval {
        rpc_id: RequestId,
        thread: ThreadId,
        shape: AnswerShape,
        event: ThreadEvent,
    },
    /// `serverRequest/resolved`: Codex no longer waits on this request.
    Resolved {
        thread: ThreadId,
        rpc_id: RequestId,
    },
    /// A server request we do not serve. It still needs an error reply or the
    /// turn waits forever.
    Unsupported {
        rpc_id: RequestId,
        method: String,
        event: CodexEvent,
    },
}

pub(crate) fn parse_line(line: &str) -> serde_json::Result<Inbound> {
    let raw = serde_json::from_str::<Value>(line)?;
    Ok(
        match serde_json::from_value::<JsonRpcMessage>(raw.clone()) {
            Ok(msg) => inbound(msg, raw),
            Err(_) => unknown(raw),
        },
    )
}

fn inbound(msg: JsonRpcMessage, raw: Value) -> Inbound {
    match msg {
        JsonRpcMessage::Response(r) => Inbound::Response {
            id: r.id,
            outcome: Ok(r.result),
        },
        JsonRpcMessage::Error(e) => Inbound::Response {
            id: e.id,
            outcome: Err(e.error.message),
        },
        JsonRpcMessage::Request(req) => {
            match ServerRequest::from_envelope(&req.method, req.params) {
                Ok(request) => approval(req.id, request, raw),
                // A known method whose params no longer fit our types.
                Err(_) => unsupported(req.id, req.method, raw),
            }
        }
        JsonRpcMessage::Notification(n) => match Notification::from_envelope(&n.method, n.params) {
            Ok(notification) => notification_event(notification, raw),
            Err(_) => Inbound::Event(thread_unknown(raw)),
        },
    }
}

/// The plain decisions the schema allows, offered when Codex lists none.
const PLAIN_DECISIONS: [&str; 4] = ["accept", "acceptForSession", "decline", "cancel"];

fn plain_decisions() -> Vec<Value> {
    PLAIN_DECISIONS.into_iter().map(Value::from).collect()
}

/// A server request we serve, before it is tagged with its id.
struct Asked {
    thread: String,
    turn: String,
    shape: AnswerShape,
    kind: ApprovalKind,
    options: Vec<Value>,
}

fn asked(request: ServerRequest) -> Option<Asked> {
    match request {
        ServerRequest::CmdExecApproval(p) => Some(Asked {
            options: match p.available_decisions {
                Some(decisions) => decisions
                    .iter()
                    .map(|d| serde_json::to_value(d).unwrap_or(Value::Null))
                    .collect(),
                None => plain_decisions(),
            },
            thread: p.thread_id,
            turn: p.turn_id,
            shape: AnswerShape::Command,
            kind: ApprovalKind::Command {
                command: p.command,
                cwd: p.cwd.map(|c| c.0),
                reason: p.reason,
            },
        }),
        ServerRequest::FileChangeApproval(p) => Some(Asked {
            thread: p.thread_id,
            turn: p.turn_id,
            shape: AnswerShape::FileChange,
            kind: ApprovalKind::FileChange {
                reason: p.reason,
                grant_root: p.grant_root,
            },
            options: plain_decisions(),
        }),
        ServerRequest::PermissionsRequestApproval(p) => {
            let requested = serde_json::to_value(&p.permissions).unwrap_or(Value::Null);
            Some(Asked {
                thread: p.thread_id,
                turn: p.turn_id,
                shape: AnswerShape::Permissions(requested.clone()),
                kind: ApprovalKind::Permissions {
                    reason: p.reason,
                    requested,
                },
                options: Vec::new(),
            })
        }
        ServerRequest::ToolRequestUserInput(p) => Some(Asked {
            thread: p.thread_id,
            turn: p.turn_id,
            shape: AnswerShape::UserInput,
            kind: ApprovalKind::UserInput {
                questions: serde_json::to_value(&p.questions).unwrap_or(Value::Null),
            },
            options: Vec::new(),
        }),
        _ => None,
    }
}

fn approval(rpc_id: RequestId, request: ServerRequest, raw: Value) -> Inbound {
    let method = String::from(request.method());
    match asked(request) {
        Some(asked) => Inbound::Approval {
            thread: ThreadId(asked.thread),
            shape: asked.shape,
            event: ThreadEvent::ApprovalRequested {
                id: ApprovalId::of(&rpc_id),
                turn: TurnId(asked.turn),
                kind: asked.kind,
                options: asked.options,
            },
            rpc_id,
        },
        None => unsupported(rpc_id, method, raw),
    }
}

fn on_thread(thread: String, event: ThreadEvent) -> Inbound {
    Inbound::Event(CodexEvent::Thread {
        thread: ThreadId(thread),
        event,
    })
}

fn notification_event(notification: Notification, raw: Value) -> Inbound {
    match notification {
        Notification::ThreadStarted(n) => on_thread(n.thread.id, ThreadEvent::Started),
        Notification::TurnStarted(n) => on_thread(
            n.thread_id,
            ThreadEvent::TurnStarted {
                turn: TurnId(n.turn.id),
            },
        ),
        Notification::TurnCompleted(n) => {
            let outcome = match n.turn.status {
                TurnStatus::Completed => TurnOutcome::Completed,
                TurnStatus::Interrupted => TurnOutcome::Interrupted,
                TurnStatus::Failed | TurnStatus::InProgress => TurnOutcome::Failed {
                    message: n.turn.error.map(|e| e.message),
                },
            };
            on_thread(
                n.thread_id,
                ThreadEvent::TurnCompleted {
                    turn: TurnId(n.turn.id),
                    outcome,
                },
            )
        }
        Notification::ServerRequestResolved(n) => Inbound::Resolved {
            thread: ThreadId(n.thread_id),
            rpc_id: n.request_id,
        },
        Notification::ItemStarted(n) => match item(n.item.payload) {
            Some(ItemKind::Agent(_)) | None => Inbound::Event(thread_unknown(raw)),
            Some(ItemKind::Tool(item)) => on_thread(
                n.thread_id,
                ThreadEvent::Item {
                    turn: TurnId(n.turn_id),
                    phase: ItemPhase::Started,
                    item,
                },
            ),
        },
        Notification::ItemCompleted(n) => match item(n.item.payload) {
            None => Inbound::Event(thread_unknown(raw)),
            Some(ItemKind::Agent(text)) => on_thread(
                n.thread_id,
                ThreadEvent::AgentMessage {
                    turn: TurnId(n.turn_id),
                    text,
                },
            ),
            Some(ItemKind::Tool(item)) => on_thread(
                n.thread_id,
                ThreadEvent::Item {
                    turn: TurnId(n.turn_id),
                    phase: ItemPhase::Completed,
                    item,
                },
            ),
        },
        _ => Inbound::Event(thread_unknown(raw)),
    }
}

/// The item kinds we map; everything else stays `Unknown`.
enum ItemKind {
    Agent(String),
    Tool(Item),
}

fn item(payload: ThreadItemPayload) -> Option<ItemKind> {
    match payload {
        ThreadItemPayload::AgentMessage { text, .. } => Some(ItemKind::Agent(text)),
        ThreadItemPayload::CommandExecution {
            id,
            command,
            status,
            exit_code,
            ..
        } => Some(ItemKind::Tool(Item::Command {
            id,
            command,
            status: wire(&status),
            exit_code,
        })),
        ThreadItemPayload::FileChange {
            id,
            changes,
            status,
        } => Some(ItemKind::Tool(Item::FileChange {
            id,
            changes: changes
                .into_iter()
                .map(|c| FileChange {
                    path: c.path,
                    kind: match c.kind {
                        PatchChangeKind::Add => ChangeKind::Add,
                        PatchChangeKind::Delete => ChangeKind::Delete,
                        PatchChangeKind::Update { .. } => ChangeKind::Update,
                    },
                })
                .collect(),
            status: wire(&status),
        })),
        _ => None,
    }
}

/// A closed Codex enum as the bare string it serializes to.
fn wire<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(Value::String(s)) => s,
        other => format!("{other:?}"),
    }
}

/// A frame with a `params.threadId` belongs to that thread even when we do
/// not map it.
fn thread_unknown(raw: Value) -> CodexEvent {
    let thread = raw
        .pointer("/params/threadId")
        .or_else(|| raw.pointer("/params/thread/id"))
        .and_then(Value::as_str);
    match thread {
        Some(thread) => CodexEvent::Thread {
            thread: ThreadId(thread.to_owned()),
            event: ThreadEvent::Unknown(raw),
        },
        None => CodexEvent::Unknown(raw),
    }
}

/// A frame that does not parse as JSON-RPC. If it still has an `id` and a
/// `method`, it is a request that needs a reply.
fn unknown(raw: Value) -> Inbound {
    match (
        raw.get("id").cloned(),
        raw.get("method").and_then(Value::as_str),
    ) {
        (Some(id), Some(method)) => match serde_json::from_value::<RequestId>(id) {
            Ok(rpc_id) => unsupported(rpc_id, method.to_owned(), raw),
            Err(_) => Inbound::Event(thread_unknown(raw)),
        },
        _ => Inbound::Event(thread_unknown(raw)),
    }
}

fn unsupported(rpc_id: RequestId, method: String, raw: Value) -> Inbound {
    Inbound::Unsupported {
        rpc_id,
        method,
        event: thread_unknown(raw),
    }
}

/// A request we originate.
#[derive(Debug, Clone)]
pub(crate) enum Call {
    Initialize,
    ModelList,
    ThreadStart(ThreadOptions),
    ThreadResume(ThreadId, ThreadOptions),
    TurnStart {
        thread: ThreadId,
        text: String,
    },
    TurnSteer {
        thread: ThreadId,
        expected_turn: TurnId,
        text: String,
    },
    TurnInterrupt {
        thread: ThreadId,
        turn: TurnId,
    },
}

impl Call {
    pub(crate) fn method(&self) -> &str {
        match self {
            Self::Initialize => methods::INITIALIZE,
            Self::ModelList => methods::MODEL_LIST,
            Self::ThreadStart(_) => methods::THREAD_START,
            Self::ThreadResume(_, _) => methods::THREAD_RESUME,
            Self::TurnStart { .. } => methods::TURN_START,
            Self::TurnSteer { .. } => methods::TURN_STEER,
            Self::TurnInterrupt { .. } => methods::TURN_INTERRUPT,
        }
    }

    fn params(&self) -> serde_json::Result<Value> {
        match self {
            Self::Initialize => serde_json::to_value(InitializeParams {
                client_info: ClientInfo {
                    name: String::from("conductor"),
                    title: None,
                    version: String::from(env!("CARGO_PKG_VERSION")),
                },
                capabilities: Some(InitializeCapabilities {
                    experimental_api: Some(true),
                    ..Default::default()
                }),
            }),
            Self::ModelList => serde_json::to_value(ModelListParams::default()),
            Self::ThreadStart(opts) => serde_json::to_value(ThreadStartParams {
                approval_policy: Some(approval_policy(opts.approval_policy)),
                sandbox: Some(sandbox(opts.sandbox)),
                cwd: Some(opts.cwd.display().to_string()),
                model: opts.model.clone(),
                config: effort_config(opts)?,
                ..Default::default()
            }),
            Self::ThreadResume(thread, opts) => serde_json::to_value(ThreadResumeParams {
                thread_id: thread.0.clone(),
                approval_policy: Some(approval_policy(opts.approval_policy)),
                sandbox: Some(sandbox(opts.sandbox)),
                cwd: Some(opts.cwd.display().to_string()),
                model: opts.model.clone(),
                config: effort_config(opts)?,
                ..Default::default()
            }),
            Self::TurnStart { thread, text } => serde_json::to_value(TurnStartParams {
                thread_id: thread.0.clone(),
                input: text_input(text),
                ..Default::default()
            }),
            Self::TurnSteer {
                thread,
                expected_turn,
                text,
            } => serde_json::to_value(TurnSteerParams {
                thread_id: thread.0.clone(),
                expected_turn_id: expected_turn.0.clone(),
                input: text_input(text),
                ..Default::default()
            }),
            Self::TurnInterrupt { thread, turn } => serde_json::to_value(TurnInterruptParams {
                thread_id: thread.0.clone(),
                turn_id: turn.0.clone(),
            }),
        }
    }
}

fn text_input(text: &str) -> Vec<UserInput> {
    Vec::from([UserInput::Text {
        text: text.to_owned(),
        text_elements: None,
    }])
}

/// The `config` override that sets the reasoning effort.
#[derive(Serialize)]
struct EffortConfig {
    model_reasoning_effort: ReasoningEffort,
}

fn effort_config(opts: &ThreadOptions) -> serde_json::Result<Option<Value>> {
    opts.effort
        .as_ref()
        .map(|effort| {
            serde_json::to_value(EffortConfig {
                model_reasoning_effort: ReasoningEffort(effort.clone()),
            })
        })
        .transpose()
}

fn approval_policy(p: ApprovalPolicy) -> AskForApproval {
    match p {
        ApprovalPolicy::Untrusted => AskForApproval::Untrusted,
        ApprovalPolicy::OnRequest => AskForApproval::OnRequest,
        ApprovalPolicy::Never => AskForApproval::Never,
    }
}

fn sandbox(s: Sandbox) -> SandboxMode {
    match s {
        Sandbox::ReadOnly => SandboxMode::Read_only,
        Sandbox::WorkspaceWrite => SandboxMode::Workspace_write,
        Sandbox::DangerFullAccess => SandboxMode::Danger_full_access,
    }
}

pub(crate) fn models(result: Value) -> serde_json::Result<Vec<ModelInfo>> {
    let list = serde_json::from_value::<codex_codes::ModelListResponse>(result)?;
    Ok(list
        .data
        .into_iter()
        .map(|m| ModelInfo {
            id: m.id,
            description: m.description,
        })
        .collect())
}

pub(crate) fn thread_id(result: Value) -> serde_json::Result<ThreadId> {
    // `thread/start` and `thread/resume` both answer `{thread: {id, ..}, ..}`.
    let r = serde_json::from_value::<codex_codes::ThreadStartResponse>(result)?;
    Ok(ThreadId(r.thread.id))
}

pub(crate) fn turn_id(result: Value) -> serde_json::Result<TurnId> {
    let r = serde_json::from_value::<codex_codes::TurnStartResponse>(result)?;
    Ok(TurnId(r.turn.id))
}

/// `userAgent` from `initialize`, such as `conductor/0.166.0 (Ubuntu ...)`.
pub(crate) fn user_agent(result: Value) -> serde_json::Result<String> {
    let r = serde_json::from_value::<codex_codes::InitializeResponse>(result)?;
    Ok(r.user_agent)
}

pub(crate) enum Outbound {
    Request {
        id: i64,
        call: Call,
    },
    Initialized,
    Answer {
        rpc_id: RequestId,
        result: Value,
    },
    /// Reply to a server request we do not serve.
    Reject {
        rpc_id: RequestId,
        message: String,
    },
}

/// Why a decision cannot answer a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotApplicable {}

/// What `permissions/requestApproval` grants back, and for how long.
#[derive(Serialize)]
struct PermissionGrant {
    permissions: Value,
    scope: GrantScope,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum GrantScope {
    Turn,
    Session,
}

/// The JSON-RPC `result` that answers an open request with `decision`.
pub(crate) fn answer(shape: &AnswerShape, decision: Decision) -> Result<Value, NotApplicable> {
    match (shape, decision) {
        (_, Decision::Raw(result)) => Ok(result),
        (AnswerShape::UserInput, _) => Err(NotApplicable {}),
        (AnswerShape::Command, decision) => encoded(command_answer(decision)),
        (AnswerShape::FileChange, decision) => encoded(file_change_answer(decision)),
        (AnswerShape::Permissions(requested), Decision::Accept) => encoded(PermissionGrant {
            permissions: requested.clone(),
            scope: GrantScope::Turn,
        }),
        (AnswerShape::Permissions(requested), Decision::AcceptForSession) => {
            encoded(PermissionGrant {
                permissions: requested.clone(),
                scope: GrantScope::Session,
            })
        }
        (AnswerShape::Permissions(_), Decision::Decline | Decision::Cancel) => {
            encoded(NoPermissions {
                permissions: Value::Object(serde_json::Map::new()),
            })
        }
    }
}

/// Declining a permissions request grants an empty profile.
#[derive(Serialize)]
struct NoPermissions {
    permissions: Value,
}

fn encoded<T: Serialize>(answer: T) -> Result<Value, NotApplicable> {
    serde_json::to_value(answer).map_err(|_| NotApplicable {})
}

fn command_answer(decision: Decision) -> CommandExecutionRequestApprovalResponse {
    match decision {
        Decision::Accept => CommandExecutionRequestApprovalResponse::accept(),
        Decision::AcceptForSession => CommandExecutionRequestApprovalResponse::accept_for_session(),
        Decision::Decline => CommandExecutionRequestApprovalResponse::decline(),
        Decision::Cancel | Decision::Raw(_) => CommandExecutionRequestApprovalResponse::cancel(),
    }
}

fn file_change_answer(decision: Decision) -> FileChangeRequestApprovalResponse {
    match decision {
        Decision::Accept => FileChangeRequestApprovalResponse::accept(),
        Decision::AcceptForSession => FileChangeRequestApprovalResponse::accept_for_session(),
        Decision::Decline => FileChangeRequestApprovalResponse::decline(),
        Decision::Cancel | Decision::Raw(_) => FileChangeRequestApprovalResponse::cancel(),
    }
}

/// JSON-RPC "method not found".
const METHOD_NOT_FOUND: i64 = 32_601_i64.wrapping_neg();

// Fields are in alphabetical order, the order `serde_json::json!` wrote them in.
#[derive(Serialize)]
struct RequestFrame {
    id: i64,
    method: String,
    params: Value,
}

#[derive(Serialize)]
struct NotificationFrame {
    method: String,
}

#[derive(Serialize)]
struct AnswerFrame {
    id: RequestId,
    result: Value,
}

#[derive(Serialize)]
struct RejectFrame {
    error: RpcError,
    id: RequestId,
}

#[derive(Serialize)]
struct RpcError {
    code: i64,
    message: String,
}

pub(crate) fn encode(msg: Outbound) -> serde_json::Result<String> {
    match msg {
        Outbound::Request { id, call } => serde_json::to_string(&RequestFrame {
            id,
            method: String::from(call.method()),
            params: call.params()?,
        }),
        Outbound::Initialized => serde_json::to_string(&NotificationFrame {
            method: String::from(methods::INITIALIZED),
        }),
        Outbound::Answer { rpc_id, result } => {
            serde_json::to_string(&AnswerFrame { id: rpc_id, result })
        }
        Outbound::Reject { rpc_id, message } => serde_json::to_string(&RejectFrame {
            error: RpcError {
                code: METHOD_NOT_FOUND,
                message,
            },
            id: rpc_id,
        }),
    }
}

#[cfg(test)]
mod tests {
    use codex_codes::RequestId;
    use serde_json::Value;

    use super::{
        AnswerShape, ApprovalId, ApprovalKind, ApprovalPolicy, Call, CodexEvent, Decision, Inbound,
        NotApplicable, Outbound, Sandbox, ThreadEvent, ThreadId, ThreadOptions, TurnId, answer,
        encode, parse_line,
    };

    fn json(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    fn sent(msg: Outbound) -> Value {
        json(&encode(msg).unwrap())
    }

    fn request(call: Call) -> Value {
        sent(Outbound::Request { id: 7, call })
    }

    fn opts() -> ThreadOptions {
        ThreadOptions {
            cwd: "/tmp/s2-scratch".into(),
            model: Some(String::from("gpt-6-luna")),
            effort: Some(String::from("low")),
            approval_policy: ApprovalPolicy::OnRequest,
            sandbox: Sandbox::ReadOnly,
        }
    }

    #[test]
    fn initialize_opts_into_the_experimental_api() {
        assert_eq!(
            request(Call::Initialize),
            json(&format!(
                r#"{{"id": 7, "method": "initialize", "params": {{
                "clientInfo": {{"name": "conductor", "version": "{}"}},
                "capabilities": {{"experimentalApi": true}}}}}}"#,
                env!("CARGO_PKG_VERSION")
            ))
        );
        assert_eq!(
            sent(Outbound::Initialized),
            json(r#"{"method": "initialized"}"#)
        );
    }

    #[test]
    fn thread_start_and_resume_carry_policy_sandbox_and_effort() {
        assert_eq!(
            request(Call::ThreadStart(opts())),
            json(
                r#"{"id": 7, "method": "thread/start", "params": {
                "approvalPolicy": "on-request", "sandbox": "read-only", "cwd": "/tmp/s2-scratch",
                "model": "gpt-6-luna", "config": {"model_reasoning_effort": "low"}}}"#
            )
        );
        assert_eq!(
            request(Call::ThreadResume(ThreadId(String::from("th_1")), opts())),
            json(
                r#"{"id": 7, "method": "thread/resume", "params": {
                "threadId": "th_1", "approvalPolicy": "on-request", "sandbox": "read-only",
                "cwd": "/tmp/s2-scratch", "model": "gpt-6-luna",
                "config": {"model_reasoning_effort": "low"}}}"#
            )
        );
    }

    #[test]
    fn turn_requests() {
        assert_eq!(
            request(Call::TurnStart {
                thread: ThreadId(String::from("th_1")),
                text: String::from("create a.txt")
            }),
            json(
                r#"{"id": 7, "method": "turn/start", "params": {
                "threadId": "th_1", "input": [{"type": "text", "text": "create a.txt"}]}}"#
            )
        );
        assert_eq!(
            request(Call::TurnSteer {
                thread: ThreadId(String::from("th_1")),
                expected_turn: TurnId(String::from("tu_2")),
                text: String::from("also b")
            }),
            json(
                r#"{"id": 7, "method": "turn/steer", "params": {
                "threadId": "th_1", "expectedTurnId": "tu_2",
                "input": [{"type": "text", "text": "also b"}]}}"#
            )
        );
        assert_eq!(
            request(Call::TurnInterrupt {
                thread: ThreadId(String::from("th_1")),
                turn: TurnId(String::from("tu_2"))
            }),
            json(
                r#"{"id": 7, "method": "turn/interrupt", "params": {"threadId": "th_1", "turnId": "tu_2"}}"#
            )
        );
        assert_eq!(
            request(Call::ModelList),
            json(r#"{"id": 7, "method": "model/list", "params": {}}"#)
        );
    }

    fn answered(shape: &AnswerShape, decision: Decision) -> Value {
        let result = answer(shape, decision).unwrap();
        sent(Outbound::Answer {
            rpc_id: RequestId::Integer(0),
            result,
        })
    }

    fn plain_answers(shape: &AnswerShape) -> Vec<Value> {
        Vec::from([
            Decision::Accept,
            Decision::AcceptForSession,
            Decision::Decline,
            Decision::Cancel,
        ])
        .into_iter()
        .map(|decision| answered(shape, decision))
        .collect()
    }

    #[test]
    fn command_and_file_change_decisions() {
        let expected = Vec::from(["accept", "acceptForSession", "decline", "cancel"])
            .into_iter()
            .map(|wire| {
                json(&format!(
                    r#"{{"id": 0, "result": {{"decision": "{wire}"}}}}"#
                ))
            })
            .collect::<Vec<Value>>();
        assert_eq!(plain_answers(&AnswerShape::Command), expected);
        assert_eq!(plain_answers(&AnswerShape::FileChange), expected);
    }

    #[test]
    fn a_native_option_is_sent_verbatim() {
        let option = json(r#"{"acceptWithExecpolicyAmendment": {"execpolicy_amendment": ["ls"]}}"#);
        assert_eq!(
            answered(
                &AnswerShape::Command,
                Decision::Raw(json(&format!(r#"{{"decision": {option}}}"#)))
            ),
            json(
                r#"{"id": 0, "result": {"decision":
                {"acceptWithExecpolicyAmendment": {"execpolicy_amendment": ["ls"]}}}}"#
            )
        );
    }

    #[test]
    fn permissions_grant_back_what_was_requested() {
        let shape = AnswerShape::Permissions(json(r#"{"network": {"enabled": true}}"#));
        assert_eq!(
            answered(&shape, Decision::Accept),
            json(
                r#"{"id": 0, "result": {"permissions": {"network": {"enabled": true}}, "scope": "turn"}}"#
            )
        );
        assert_eq!(
            answered(&shape, Decision::AcceptForSession),
            json(
                r#"{"id": 0, "result": {"permissions": {"network": {"enabled": true}}, "scope": "session"}}"#
            )
        );
        assert_eq!(
            answered(&shape, Decision::Decline),
            json(r#"{"id": 0, "result": {"permissions": {}}}"#)
        );
    }

    #[test]
    fn user_input_needs_a_raw_answer() {
        assert_eq!(
            answer(&AnswerShape::UserInput, Decision::Accept),
            Err(NotApplicable {})
        );
    }

    #[test]
    fn unsupported_request_is_rejected_with_method_not_found() {
        assert_eq!(
            sent(Outbound::Reject {
                rpc_id: RequestId::Integer(4),
                message: String::from("conductor does not handle item/tool/call"),
            }),
            json(
                r#"{"id": 4, "error": {"code": -32601,
                "message": "conductor does not handle item/tool/call"}}"#
            )
        );
    }

    #[test]
    fn command_approval_keeps_codex_options_in_order() {
        let line = r#"{"method":"item/commandExecution/requestApproval","id":0,"params":{"kind":"command","threadId":"th_1","turnId":"tu_1","itemId":"exec-1","startedAtMs":1,"reason":"May I?","command":"printf hi > a.txt","cwd":"/tmp/s","availableDecisions":["accept",{"acceptWithExecpolicyAmendment":{"execpolicy_amendment":["printf"]}},"cancel"]}}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Inbound::Approval {
                rpc_id: RequestId::Integer(0),
                thread: ThreadId(String::from("th_1")),
                shape: AnswerShape::Command,
                event: ThreadEvent::ApprovalRequested {
                    id: ApprovalId(String::from("0")),
                    turn: TurnId(String::from("tu_1")),
                    kind: ApprovalKind::Command {
                        command: Some(String::from("printf hi > a.txt")),
                        cwd: Some(String::from("/tmp/s")),
                        reason: Some(String::from("May I?")),
                    },
                    options: Vec::from([
                        json(r#""accept""#),
                        json(
                            r#"{"acceptWithExecpolicyAmendment": {"execpolicy_amendment": ["printf"]}}"#
                        ),
                        json(r#""cancel""#),
                    ]),
                },
            }
        );
    }

    #[test]
    fn unknown_notification_keeps_its_thread() {
        let line = r#"{"method":"thread/brandNew","params":{"threadId":"th_1","x":1}}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Inbound::Event(CodexEvent::Thread {
                thread: ThreadId(String::from("th_1")),
                event: ThreadEvent::Unknown(json(line)),
            })
        );
    }

    #[test]
    fn known_method_with_drifted_params_becomes_unknown() {
        let line = r#"{"method":"turn/completed","params":{"threadId":"th_1","turn":{"id":"tu_1","items":[],"status":"paused"}}}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Inbound::Event(CodexEvent::Thread {
                thread: ThreadId(String::from("th_1")),
                event: ThreadEvent::Unknown(json(line)),
            })
        );
    }

    #[test]
    fn unknown_server_request_still_yields_its_id() {
        let line = r#"{"method":"item/frobnicate","id":9,"params":{"threadId":"th_1"}}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Inbound::Unsupported {
                rpc_id: RequestId::Integer(9),
                method: String::from("item/frobnicate"),
                event: CodexEvent::Thread {
                    thread: ThreadId(String::from("th_1")),
                    event: ThreadEvent::Unknown(json(line)),
                },
            }
        );
    }

    #[test]
    fn resolved_names_the_request_id() {
        let line =
            r#"{"method":"serverRequest/resolved","params":{"threadId":"th_1","requestId":0}}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Inbound::Resolved {
                thread: ThreadId(String::from("th_1")),
                rpc_id: RequestId::Integer(0),
            }
        );
    }
}
