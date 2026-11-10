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
use serde_json::{Value, json};

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

impl std::fmt::Display for ApprovalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&RequestId> for ApprovalId {
    fn from(id: &RequestId) -> Self {
        Self(id.to_string())
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    /// `add`, `delete` or `update`.
    pub kind: &'static str,
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
    let raw: Value = serde_json::from_str(line)?;
    let Ok(msg) = serde_json::from_value::<JsonRpcMessage>(raw.clone()) else {
        return Ok(unknown(raw));
    };
    Ok(match msg {
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
    })
}

fn approval(rpc_id: RequestId, request: ServerRequest, raw: Value) -> Inbound {
    let id = ApprovalId::from(&rpc_id);
    let plain = || {
        ["accept", "acceptForSession", "decline", "cancel"]
            .map(Value::from)
            .to_vec()
    };
    let (thread, turn, shape, kind, options) = match request {
        ServerRequest::CmdExecApproval(p) => {
            let options = match p.available_decisions {
                Some(decisions) => decisions
                    .iter()
                    .map(|d| serde_json::to_value(d).unwrap_or(Value::Null))
                    .collect(),
                None => plain(),
            };
            (
                p.thread_id,
                p.turn_id,
                AnswerShape::Command,
                ApprovalKind::Command {
                    command: p.command,
                    cwd: p.cwd.map(|c| c.0),
                    reason: p.reason,
                },
                options,
            )
        }
        ServerRequest::FileChangeApproval(p) => (
            p.thread_id,
            p.turn_id,
            AnswerShape::FileChange,
            ApprovalKind::FileChange {
                reason: p.reason,
                grant_root: p.grant_root,
            },
            plain(),
        ),
        ServerRequest::PermissionsRequestApproval(p) => {
            let requested = serde_json::to_value(&p.permissions).unwrap_or(Value::Null);
            (
                p.thread_id,
                p.turn_id,
                AnswerShape::Permissions(requested.clone()),
                ApprovalKind::Permissions {
                    reason: p.reason,
                    requested,
                },
                Vec::new(),
            )
        }
        ServerRequest::ToolRequestUserInput(p) => (
            p.thread_id,
            p.turn_id,
            AnswerShape::UserInput,
            ApprovalKind::UserInput {
                questions: serde_json::to_value(&p.questions).unwrap_or(Value::Null),
            },
            Vec::new(),
        ),
        other => {
            return unsupported(rpc_id, other.method().to_owned(), raw);
        }
    };
    Inbound::Approval {
        rpc_id,
        thread: ThreadId(thread),
        shape,
        event: ThreadEvent::ApprovalRequested {
            id,
            turn: TurnId(turn),
            kind,
            options,
        },
    }
}

fn notification_event(notification: Notification, raw: Value) -> Inbound {
    let on = |thread: String, event| {
        Inbound::Event(CodexEvent::Thread {
            thread: ThreadId(thread),
            event,
        })
    };
    match notification {
        Notification::ThreadStarted(n) => on(n.thread.id, ThreadEvent::Started),
        Notification::TurnStarted(n) => on(
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
            on(
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
            Some(ItemKind::Tool(item)) => on(
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
            Some(ItemKind::Agent(text)) => on(
                n.thread_id,
                ThreadEvent::AgentMessage {
                    turn: TurnId(n.turn_id),
                    text,
                },
            ),
            Some(ItemKind::Tool(item)) => on(
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
    Some(match payload {
        ThreadItemPayload::AgentMessage { text, .. } => ItemKind::Agent(text),
        ThreadItemPayload::CommandExecution {
            id,
            command,
            status,
            exit_code,
            ..
        } => ItemKind::Tool(Item::Command {
            id,
            command,
            status: wire(&status),
            exit_code,
        }),
        ThreadItemPayload::FileChange {
            id,
            changes,
            status,
        } => ItemKind::Tool(Item::FileChange {
            id,
            changes: changes
                .into_iter()
                .map(|c| FileChange {
                    path: c.path,
                    kind: match c.kind {
                        PatchChangeKind::Add => "add",
                        PatchChangeKind::Delete => "delete",
                        PatchChangeKind::Update { .. } => "update",
                    },
                })
                .collect(),
            status: wire(&status),
        }),
        _ => return None,
    })
}

/// A closed Codex enum as the bare string it serializes to.
fn wire<T: serde::Serialize>(v: &T) -> String {
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
    pub(crate) fn method(&self) -> &'static str {
        match self {
            Self::Initialize => methods::INITIALIZE,
            Self::ModelList => methods::MODEL_LIST,
            Self::ThreadStart(_) => methods::THREAD_START,
            Self::ThreadResume(..) => methods::THREAD_RESUME,
            Self::TurnStart { .. } => methods::TURN_START,
            Self::TurnSteer { .. } => methods::TURN_STEER,
            Self::TurnInterrupt { .. } => methods::TURN_INTERRUPT,
        }
    }

    fn params(&self) -> serde_json::Result<Value> {
        let text_input = |text: &str| {
            vec![UserInput::Text {
                text: text.to_owned(),
                text_elements: None,
            }]
        };
        match self {
            Self::Initialize => serde_json::to_value(InitializeParams {
                client_info: ClientInfo {
                    name: "conductor".into(),
                    title: None,
                    version: env!("CARGO_PKG_VERSION").into(),
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
                config: effort_config(opts),
                ..Default::default()
            }),
            Self::ThreadResume(thread, opts) => serde_json::to_value(ThreadResumeParams {
                thread_id: thread.0.clone(),
                approval_policy: Some(approval_policy(opts.approval_policy)),
                sandbox: Some(sandbox(opts.sandbox)),
                cwd: Some(opts.cwd.display().to_string()),
                model: opts.model.clone(),
                config: effort_config(opts),
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

fn effort_config(opts: &ThreadOptions) -> Option<Value> {
    opts.effort
        .as_ref()
        .map(|e| json!({"model_reasoning_effort": ReasoningEffort(e.clone())}))
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
    let list: codex_codes::ModelListResponse = serde_json::from_value(result)?;
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
    let r: codex_codes::ThreadStartResponse = serde_json::from_value(result)?;
    Ok(ThreadId(r.thread.id))
}

pub(crate) fn turn_id(result: Value) -> serde_json::Result<TurnId> {
    let r: codex_codes::TurnStartResponse = serde_json::from_value(result)?;
    Ok(TurnId(r.turn.id))
}

/// `userAgent` from `initialize`, such as `conductor/0.166.0 (Ubuntu ...)`.
pub(crate) fn user_agent(result: Value) -> serde_json::Result<String> {
    let r: codex_codes::InitializeResponse = serde_json::from_value(result)?;
    Ok(r.user_agent)
}

pub(crate) enum Outbound<'a> {
    Request {
        id: i64,
        call: &'a Call,
    },
    Initialized,
    Answer {
        rpc_id: &'a RequestId,
        result: Value,
    },
    /// Reply to a server request we do not serve.
    Reject {
        rpc_id: &'a RequestId,
        message: &'a str,
    },
}

/// Why a decision cannot answer a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotApplicable;

/// The JSON-RPC `result` that answers an open request with `decision`.
pub(crate) fn answer(shape: &AnswerShape, decision: Decision) -> Result<Value, NotApplicable> {
    let encoded = match (shape, decision) {
        (_, Decision::Raw(result)) => return Ok(result),
        (AnswerShape::UserInput, _) => return Err(NotApplicable),
        (AnswerShape::Command, d) => serde_json::to_value(match d {
            Decision::Accept => CommandExecutionRequestApprovalResponse::accept(),
            Decision::AcceptForSession => {
                CommandExecutionRequestApprovalResponse::accept_for_session()
            }
            Decision::Decline => CommandExecutionRequestApprovalResponse::decline(),
            _ => CommandExecutionRequestApprovalResponse::cancel(),
        }),
        (AnswerShape::FileChange, d) => serde_json::to_value(match d {
            Decision::Accept => FileChangeRequestApprovalResponse::accept(),
            Decision::AcceptForSession => FileChangeRequestApprovalResponse::accept_for_session(),
            Decision::Decline => FileChangeRequestApprovalResponse::decline(),
            _ => FileChangeRequestApprovalResponse::cancel(),
        }),
        (AnswerShape::Permissions(requested), Decision::Accept) => {
            Ok(json!({"permissions": requested, "scope": "turn"}))
        }
        (AnswerShape::Permissions(requested), Decision::AcceptForSession) => {
            Ok(json!({"permissions": requested, "scope": "session"}))
        }
        (AnswerShape::Permissions(_), _) => Ok(json!({"permissions": {}})),
    };
    encoded.map_err(|_| NotApplicable)
}

pub(crate) fn encode(msg: Outbound<'_>) -> serde_json::Result<String> {
    let value = match msg {
        Outbound::Request { id, call } => json!({
            "id": id,
            "method": call.method(),
            "params": call.params()?,
        }),
        Outbound::Initialized => json!({"method": methods::INITIALIZED}),
        Outbound::Answer { rpc_id, result } => json!({"id": rpc_id, "result": result}),
        Outbound::Reject { rpc_id, message } => json!({
            "id": rpc_id,
            "error": {"code": -32601, "message": message},
        }),
    };
    serde_json::to_string(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(msg: Outbound<'_>) -> Value {
        serde_json::from_str(&encode(msg).unwrap()).unwrap()
    }

    fn request(call: Call) -> Value {
        sent(Outbound::Request { id: 7, call: &call })
    }

    fn opts() -> ThreadOptions {
        ThreadOptions {
            cwd: "/tmp/s2-scratch".into(),
            model: Some("gpt-6-luna".into()),
            effort: Some("low".into()),
            approval_policy: ApprovalPolicy::OnRequest,
            sandbox: Sandbox::ReadOnly,
        }
    }

    #[test]
    fn initialize_opts_into_the_experimental_api() {
        assert_eq!(
            request(Call::Initialize),
            json!({"id": 7, "method": "initialize", "params": {
                "clientInfo": {"name": "conductor", "version": env!("CARGO_PKG_VERSION")},
                "capabilities": {"experimentalApi": true}}})
        );
        assert_eq!(
            sent(Outbound::Initialized),
            json!({"method": "initialized"})
        );
    }

    #[test]
    fn thread_start_and_resume_carry_policy_sandbox_and_effort() {
        assert_eq!(
            request(Call::ThreadStart(opts())),
            json!({"id": 7, "method": "thread/start", "params": {
                "approvalPolicy": "on-request", "sandbox": "read-only", "cwd": "/tmp/s2-scratch",
                "model": "gpt-6-luna", "config": {"model_reasoning_effort": "low"}}})
        );
        assert_eq!(
            request(Call::ThreadResume(ThreadId("th_1".into()), opts())),
            json!({"id": 7, "method": "thread/resume", "params": {
                "threadId": "th_1", "approvalPolicy": "on-request", "sandbox": "read-only",
                "cwd": "/tmp/s2-scratch", "model": "gpt-6-luna",
                "config": {"model_reasoning_effort": "low"}}})
        );
    }

    #[test]
    fn turn_requests() {
        assert_eq!(
            request(Call::TurnStart {
                thread: ThreadId("th_1".into()),
                text: "create a.txt".into()
            }),
            json!({"id": 7, "method": "turn/start", "params": {
                "threadId": "th_1", "input": [{"type": "text", "text": "create a.txt"}]}})
        );
        assert_eq!(
            request(Call::TurnSteer {
                thread: ThreadId("th_1".into()),
                expected_turn: TurnId("tu_2".into()),
                text: "also b".into()
            }),
            json!({"id": 7, "method": "turn/steer", "params": {
                "threadId": "th_1", "expectedTurnId": "tu_2",
                "input": [{"type": "text", "text": "also b"}]}})
        );
        assert_eq!(
            request(Call::TurnInterrupt {
                thread: ThreadId("th_1".into()),
                turn: TurnId("tu_2".into())
            }),
            json!({"id": 7, "method": "turn/interrupt", "params": {"threadId": "th_1", "turnId": "tu_2"}})
        );
        assert_eq!(
            request(Call::ModelList),
            json!({"id": 7, "method": "model/list", "params": {}})
        );
    }

    fn answered(shape: &AnswerShape, decision: Decision) -> Value {
        let result = answer(shape, decision).unwrap();
        sent(Outbound::Answer {
            rpc_id: &RequestId::Integer(0),
            result,
        })
    }

    #[test]
    fn command_and_file_change_decisions() {
        for shape in [AnswerShape::Command, AnswerShape::FileChange] {
            for (decision, wire) in [
                (Decision::Accept, "accept"),
                (Decision::AcceptForSession, "acceptForSession"),
                (Decision::Decline, "decline"),
                (Decision::Cancel, "cancel"),
            ] {
                assert_eq!(
                    answered(&shape, decision),
                    json!({"id": 0, "result": {"decision": wire}})
                );
            }
        }
    }

    #[test]
    fn a_native_option_is_sent_verbatim() {
        let option = json!({"acceptWithExecpolicyAmendment": {"execpolicy_amendment": ["ls"]}});
        assert_eq!(
            answered(
                &AnswerShape::Command,
                Decision::Raw(json!({"decision": option}))
            ),
            json!({"id": 0, "result": {"decision":
                {"acceptWithExecpolicyAmendment": {"execpolicy_amendment": ["ls"]}}}})
        );
    }

    #[test]
    fn permissions_grant_back_what_was_requested() {
        let shape = AnswerShape::Permissions(json!({"network": {"enabled": true}}));
        assert_eq!(
            answered(&shape, Decision::Accept),
            json!({"id": 0, "result": {"permissions": {"network": {"enabled": true}}, "scope": "turn"}})
        );
        assert_eq!(
            answered(&shape, Decision::AcceptForSession),
            json!({"id": 0, "result": {"permissions": {"network": {"enabled": true}}, "scope": "session"}})
        );
        assert_eq!(
            answered(&shape, Decision::Decline),
            json!({"id": 0, "result": {"permissions": {}}})
        );
    }

    #[test]
    fn user_input_needs_a_raw_answer() {
        assert_eq!(
            answer(&AnswerShape::UserInput, Decision::Accept),
            Err(NotApplicable)
        );
    }

    #[test]
    fn unsupported_request_is_rejected_with_method_not_found() {
        assert_eq!(
            sent(Outbound::Reject {
                rpc_id: &RequestId::Integer(4),
                message: "conductor does not handle item/tool/call",
            }),
            json!({"id": 4, "error": {"code": -32601,
                "message": "conductor does not handle item/tool/call"}})
        );
    }

    #[test]
    fn command_approval_keeps_codex_options_in_order() {
        let line = r#"{"method":"item/commandExecution/requestApproval","id":0,"params":{"kind":"command","threadId":"th_1","turnId":"tu_1","itemId":"exec-1","startedAtMs":1,"reason":"May I?","command":"printf hi > a.txt","cwd":"/tmp/s","availableDecisions":["accept",{"acceptWithExecpolicyAmendment":{"execpolicy_amendment":["printf"]}},"cancel"]}}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Inbound::Approval {
                rpc_id: RequestId::Integer(0),
                thread: ThreadId("th_1".into()),
                shape: AnswerShape::Command,
                event: ThreadEvent::ApprovalRequested {
                    id: ApprovalId("0".into()),
                    turn: TurnId("tu_1".into()),
                    kind: ApprovalKind::Command {
                        command: Some("printf hi > a.txt".into()),
                        cwd: Some("/tmp/s".into()),
                        reason: Some("May I?".into()),
                    },
                    options: vec![
                        json!("accept"),
                        json!({"acceptWithExecpolicyAmendment": {"execpolicy_amendment": ["printf"]}}),
                        json!("cancel"),
                    ],
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
                thread: ThreadId("th_1".into()),
                event: ThreadEvent::Unknown(serde_json::from_str(line).unwrap()),
            })
        );
    }

    #[test]
    fn known_method_with_drifted_params_becomes_unknown() {
        let line = r#"{"method":"turn/completed","params":{"threadId":"th_1","turn":{"id":"tu_1","items":[],"status":"paused"}}}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Inbound::Event(CodexEvent::Thread {
                thread: ThreadId("th_1".into()),
                event: ThreadEvent::Unknown(serde_json::from_str(line).unwrap()),
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
                method: "item/frobnicate".into(),
                event: CodexEvent::Thread {
                    thread: ThreadId("th_1".into()),
                    event: ThreadEvent::Unknown(serde_json::from_str(line).unwrap()),
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
                thread: ThreadId("th_1".into()),
                rpc_id: RequestId::Integer(0),
            }
        );
    }
}
