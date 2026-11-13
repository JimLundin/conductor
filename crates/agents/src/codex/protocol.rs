//! The stdio boundary. Each app-server stdout line is parsed into the few
//! JSON-RPC messages we read and mapped into our own types here, and every line
//! we write is built here. Methods and fields we do not model are ignored or
//! passed on as `Unknown`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use proto::{
    Answer, Change, Effort, FileChange, ModelChoice, ModelId, Models, Question, QuestionOption,
    Selection, ToolCallId, ToolKind, ToolOutcome, ToolPhase, TurnOutcome,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Exit;

pub(crate) const INITIALIZE: &str = "initialize";

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
        ApprovalId(match id {
            RequestId::Integer(n) => n.to_string(),
            RequestId::String(s) => s.clone(),
        })
    }
}

/// A JSON-RPC id, which Codex sends as a number or a string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum RequestId {
    Integer(i64),
    String(String),
}

impl RequestId {
    fn read(id: &Value) -> Option<RequestId> {
        serde_json::from_value(id.clone()).ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalPolicy {
    Untrusted,
    OnRequest,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
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
    /// Passed as the `model_reasoning_effort` config override.
    pub effort: Option<Effort>,
    pub approval_policy: ApprovalPolicy,
    pub sandbox: Sandbox,
}

/// The two item notification methods on the provider wire.
#[derive(Debug, Clone, Copy)]
enum ItemPhase {
    Started,
    Completed,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ChangeKind {
    Add,
    Delete,
    Update,
}

#[derive(Deserialize)]
struct WireFileChange {
    path: String,
    kind: ChangeKind,
}

/// What a permissions request asks the sandbox to allow beyond its own.
/// Codex's own names for special places (`/tmp`, the project roots) are kept
/// as they read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionAsk {
    Network,
    Read(Vec<String>),
    Write(Vec<String>),
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
        asks: Vec<PermissionAsk>,
    },
    /// `item/tool/requestUserInput`; answer it with [`Decision::Answer`].
    UserInput { questions: Vec<Question> },
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
    /// A piece of an agent message as it streams; `AgentMessage` follows with the whole text.
    AgentDelta {
        turn: TurnId,
        text: String,
    },
    /// A finished agent message, not a delta.
    AgentMessage {
        turn: TurnId,
        text: String,
    },
    /// A piece of a reasoning summary as it streams; `Reasoning` follows.
    ReasoningDelta {
        turn: TurnId,
        text: String,
    },
    /// A finished reasoning item: its summary parts, joined. Empty when the
    /// model gave none.
    Reasoning {
        turn: TurnId,
        text: String,
    },
    ToolCall {
        turn: TurnId,
        call: ToolCallId,
        phase: ToolPhase,
    },
    ApprovalRequested {
        id: ApprovalId,
        turn: TurnId,
        /// The item the request is about, as in its `ToolCall` event.
        item: String,
        kind: ApprovalKind,
        /// Codex's own decision values, verbatim. For a command this is
        /// `availableDecisions`; without it, and for a file change, it is the
        /// four plain decisions the schema allows. Empty for permissions and
        /// user input, whose answers are not a decision.
        options: Vec<Value>,
    },
    /// Codex asked for an answer to keep secret (`isSecret`). The host can't
    /// keep one: answers are logged and replayed to every client. So the
    /// server actor refused the request, and the turn goes on without it.
    SecretQuestionDeclined {
        turn: TurnId,
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
        status: Exit,
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
    /// Sent verbatim as the JSON-RPC `result`, for a native option.
    Raw(Value),
    /// The answer to a user-input request, one per question.
    Answer(Vec<Answer>),
}

/// What an answer to an open request has to look like.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AnswerShape {
    Command,
    FileChange,
    /// Holds the requested profile, which an accept grants back.
    Permissions(Value),
    /// Holds the questions' ids, which key the answers.
    UserInput(Vec<String>),
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
    /// A user-input request with a secret question, which gets an error
    /// reply. `event` names no question.
    Declined {
        rpc_id: RequestId,
        event: CodexEvent,
    },
}

/// A JSON-RPC message: a request has an `id` and a `method`, a response an
/// `id` and a `result` or an `error`, and a notification only a `method`.
#[derive(Deserialize)]
struct Envelope {
    id: Option<Value>,
    method: Option<String>,
    params: Option<Value>,
    result: Option<Value>,
    error: Option<RpcError>,
}

/// The server requests we serve, by `method`, with what each asks.
#[derive(Deserialize)]
#[serde(tag = "method", content = "params")]
enum ServerRequest {
    #[serde(rename = "item/commandExecution/requestApproval")]
    Command {
        command: Option<String>,
        cwd: Option<String>,
        reason: Option<String>,
        #[serde(rename = "availableDecisions")]
        available_decisions: Option<Vec<Value>>,
    },
    #[serde(rename = "item/fileChange/requestApproval")]
    FileChange {
        reason: Option<String>,
        #[serde(rename = "grantRoot")]
        grant_root: Option<String>,
    },
    #[serde(rename = "item/permissions/requestApproval")]
    Permissions {
        reason: Option<String>,
        permissions: Value,
    },
    #[serde(rename = "item/tool/requestUserInput")]
    UserInput { questions: Vec<WireQuestion> },
}

/// A user-input question. Codex lets every question take typed text
/// (`isOther`), and offers no multiple choice.
#[derive(Deserialize)]
struct WireQuestion {
    id: String,
    header: String,
    question: String,
    options: Option<Vec<QuestionOption>>,
    #[serde(rename = "isSecret")]
    is_secret: Option<bool>,
}

/// Where a server request belongs, common to every request we serve.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Target {
    thread_id: String,
    turn_id: String,
    item_id: String,
}

/// The notifications we map, by `method`.
#[derive(Deserialize)]
#[serde(tag = "method", content = "params")]
enum Notification {
    #[serde(rename = "thread/started")]
    ThreadStarted { thread: Id },
    #[serde(rename = "turn/started")]
    TurnStarted {
        #[serde(rename = "threadId")]
        thread_id: String,
        turn: Turn,
    },
    #[serde(rename = "turn/completed")]
    TurnCompleted {
        #[serde(rename = "threadId")]
        thread_id: String,
        turn: Turn,
    },
    #[serde(rename = "item/agentMessage/delta")]
    AgentMessageDelta {
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "turnId")]
        turn_id: String,
        delta: String,
    },
    #[serde(rename = "item/reasoning/summaryTextDelta")]
    ReasoningSummaryTextDelta {
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "turnId")]
        turn_id: String,
        delta: String,
    },
    #[serde(rename = "serverRequest/resolved")]
    ServerRequestResolved {
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "requestId")]
        request_id: Value,
    },
    #[serde(rename = "item/started")]
    ItemStarted {
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "turnId")]
        turn_id: String,
        item: ThreadItem,
    },
    #[serde(rename = "item/completed")]
    ItemCompleted {
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "turnId")]
        turn_id: String,
        item: ThreadItem,
    },
}

#[derive(Deserialize)]
struct Id {
    id: String,
}

#[derive(Deserialize)]
struct Turn {
    id: String,
    status: TurnStatus,
    error: Option<TurnError>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum TurnStatus {
    Completed,
    Interrupted,
    Failed,
    InProgress,
}

#[derive(Deserialize)]
struct TurnError {
    message: String,
}

/// The item types we map; any other makes its notification `Unknown`.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ThreadItem {
    AgentMessage {
        text: String,
    },
    Reasoning {
        summary: Vec<String>,
    },
    CommandExecution {
        id: String,
        command: String,
        status: String,
    },
    FileChange {
        id: String,
        changes: Vec<WireFileChange>,
        status: String,
    },
}

/// What a permissions request asks for, as far as we show it.
#[derive(Deserialize)]
struct PermissionProfile {
    network: Option<Network>,
    #[serde(rename = "fileSystem")]
    file_system: Option<FileSystem>,
}

#[derive(Deserialize)]
struct Network {
    enabled: Option<bool>,
}

#[derive(Deserialize)]
struct FileSystem {
    read: Option<Vec<String>>,
    write: Option<Vec<String>>,
    entries: Option<Vec<FileSystemEntry>>,
}

#[derive(Deserialize)]
struct FileSystemEntry {
    access: Access,
    path: Place,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Access {
    Read,
    Write,
    Deny,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Place {
    Path {
        path: String,
    },
    #[serde(rename = "glob_pattern")]
    GlobPattern {
        pattern: String,
    },
    Special {
        value: SpecialPlace,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum SpecialPlace {
    Root,
    Minimal,
    ProjectRoots {
        subpath: Option<String>,
    },
    Tmpdir,
    SlashTmp,
    Unknown {
        path: String,
        subpath: Option<String>,
    },
}

pub(crate) fn parse_line(line: &str) -> serde_json::Result<Inbound> {
    let raw = serde_json::from_str::<Value>(line)?;
    Ok(match serde_json::from_value::<Envelope>(raw.clone()) {
        Ok(envelope) => inbound(envelope, raw),
        Err(_) => Inbound::Event(thread_unknown(raw)),
    })
}

fn inbound(envelope: Envelope, raw: Value) -> Inbound {
    let Envelope {
        id,
        method,
        params,
        result,
        error,
    } = envelope;
    match (id.as_ref().and_then(RequestId::read), method, result, error) {
        (Some(id), Some(method), _, _) => request(id, method, params, raw),
        (Some(id), None, Some(result), _) => Inbound::Response {
            id,
            outcome: Ok(result),
        },
        (Some(id), None, None, Some(error)) => Inbound::Response {
            id,
            outcome: Err(error.message),
        },
        // `"result": null` reads as no result.
        (Some(id), None, None, None) => Inbound::Response {
            id,
            outcome: Ok(Value::Null),
        },
        (None, Some(_), _, _) => notification(raw),
        (None, None, _, _) => Inbound::Event(thread_unknown(raw)),
    }
}

/// The plain decisions the schema allows, offered when Codex lists none.
const PLAIN_DECISIONS: &[&str] = &["accept", "acceptForSession", "decline", "cancel"];

fn plain_decisions() -> Vec<Value> {
    PLAIN_DECISIONS.iter().copied().map(Value::from).collect()
}

/// What we do with a server request we read.
enum Served {
    Asked(Asked),
    /// A user-input request with a secret question, which we decline.
    Secret,
}

/// A server request we serve, before it is tagged with its id.
struct Asked {
    shape: AnswerShape,
    kind: ApprovalKind,
    options: Vec<Value>,
}

fn permission_asks(profile: PermissionProfile) -> Vec<PermissionAsk> {
    let network = profile
        .network
        .and_then(|network| network.enabled)
        .and_then(|enabled| enabled.then_some(PermissionAsk::Network));
    let (read, write) = match profile.file_system {
        Some(file_system) => (
            listed(&file_system.read, &file_system.entries, &Access::Read),
            listed(&file_system.write, &file_system.entries, &Access::Write),
        ),
        None => (Vec::new(), Vec::new()),
    };
    network
        .into_iter()
        .chain(
            Some(read)
                .filter(|paths| !paths.is_empty())
                .map(PermissionAsk::Read),
        )
        .chain(
            Some(write)
                .filter(|paths| !paths.is_empty())
                .map(PermissionAsk::Write),
        )
        .collect()
}

/// The places a profile asks `access` to, in its plain lists and its entries.
fn listed(
    plain: &Option<Vec<String>>,
    entries: &Option<Vec<FileSystemEntry>>,
    access: &Access,
) -> Vec<String> {
    plain
        .iter()
        .flatten()
        .cloned()
        .chain(
            entries
                .iter()
                .flatten()
                .filter(|entry| &entry.access == access)
                .map(|entry| place(&entry.path)),
        )
        .collect()
}

fn place(path: &Place) -> String {
    match path {
        Place::Path { path } => path.clone(),
        Place::GlobPattern { pattern } => pattern.clone(),
        Place::Special { value } => match value {
            SpecialPlace::Root => String::from("/"),
            SpecialPlace::Minimal => String::from("the minimal system paths"),
            SpecialPlace::ProjectRoots { subpath: None } => String::from("the project roots"),
            SpecialPlace::ProjectRoots {
                subpath: Some(subpath),
            } => format!("{subpath} in the project roots"),
            SpecialPlace::Tmpdir => String::from("$TMPDIR"),
            SpecialPlace::SlashTmp => String::from("/tmp"),
            SpecialPlace::Unknown {
                path,
                subpath: None,
            } => path.clone(),
            SpecialPlace::Unknown {
                path,
                subpath: Some(subpath),
            } => format!("{path}/{subpath}"),
        },
    }
}

fn served(request: ServerRequest) -> serde_json::Result<Served> {
    match request {
        ServerRequest::UserInput { questions } => match questions
            .iter()
            .position(|question| question.is_secret == Some(true))
        {
            Some(_) => Ok(Served::Secret),
            None => asked(ServerRequest::UserInput { questions }).map(Served::Asked),
        },
        request => asked(request).map(Served::Asked),
    }
}

fn asked(request: ServerRequest) -> serde_json::Result<Asked> {
    Ok(match request {
        ServerRequest::Command {
            command,
            cwd,
            reason,
            available_decisions,
        } => Asked {
            shape: AnswerShape::Command,
            kind: ApprovalKind::Command {
                command,
                cwd,
                reason,
            },
            options: available_decisions.unwrap_or_else(plain_decisions),
        },
        ServerRequest::FileChange { reason, grant_root } => Asked {
            shape: AnswerShape::FileChange,
            kind: ApprovalKind::FileChange { reason, grant_root },
            options: plain_decisions(),
        },
        ServerRequest::Permissions {
            reason,
            permissions,
        } => Asked {
            kind: ApprovalKind::Permissions {
                reason,
                asks: permission_asks(serde_json::from_value(permissions.clone())?),
            },
            shape: AnswerShape::Permissions(permissions),
            options: Vec::new(),
        },
        ServerRequest::UserInput { questions } => Asked {
            shape: AnswerShape::UserInput(
                questions
                    .iter()
                    .map(|question| question.id.clone())
                    .collect(),
            ),
            kind: ApprovalKind::UserInput {
                questions: questions
                    .into_iter()
                    .map(|question| Question {
                        header: question.header,
                        text: question.question,
                        options: question.options.unwrap_or_default(),
                        selection: Selection::One,
                    })
                    .collect(),
            },
            options: Vec::new(),
        },
    })
}

/// A request whose method we don't serve, or whose params don't fit, still
/// gets an error reply.
fn request(rpc_id: RequestId, method: String, params: Option<Value>, raw: Value) -> Inbound {
    let served = serde_json::from_value::<ServerRequest>(raw.clone())
        .and_then(served)
        .and_then(|served| {
            serde_json::from_value::<Target>(params.unwrap_or(Value::Null))
                .map(|target| (served, target))
        });
    match served {
        Ok((Served::Secret, target)) => Inbound::Declined {
            rpc_id,
            event: CodexEvent::Thread {
                thread: ThreadId(target.thread_id),
                event: ThreadEvent::SecretQuestionDeclined {
                    turn: TurnId(target.turn_id),
                },
            },
        },
        Ok((Served::Asked(asked), target)) => Inbound::Approval {
            thread: ThreadId(target.thread_id),
            shape: asked.shape,
            event: ThreadEvent::ApprovalRequested {
                id: ApprovalId::of(&rpc_id),
                turn: TurnId(target.turn_id),
                item: target.item_id,
                kind: asked.kind,
                options: asked.options,
            },
            rpc_id,
        },
        Err(_) => Inbound::Unsupported {
            rpc_id,
            method,
            event: thread_unknown(raw),
        },
    }
}

fn on_thread(thread: String, event: ThreadEvent) -> Inbound {
    Inbound::Event(CodexEvent::Thread {
        thread: ThreadId(thread),
        event,
    })
}

fn notification(raw: Value) -> Inbound {
    match serde_json::from_value::<Notification>(raw.clone()) {
        Ok(Notification::ThreadStarted { thread }) => on_thread(thread.id, ThreadEvent::Started),
        Ok(Notification::TurnStarted { thread_id, turn }) => on_thread(
            thread_id,
            ThreadEvent::TurnStarted {
                turn: TurnId(turn.id),
            },
        ),
        Ok(Notification::TurnCompleted { thread_id, turn }) => {
            let outcome = match turn.status {
                TurnStatus::Completed => TurnOutcome::Completed,
                TurnStatus::Interrupted => TurnOutcome::Interrupted,
                TurnStatus::Failed | TurnStatus::InProgress => TurnOutcome::Failed {
                    message: turn
                        .error
                        .map(|e| e.message)
                        .unwrap_or_else(|| String::from("the turn failed")),
                },
            };
            on_thread(
                thread_id,
                ThreadEvent::TurnCompleted {
                    turn: TurnId(turn.id),
                    outcome,
                },
            )
        }
        Ok(Notification::AgentMessageDelta {
            thread_id,
            turn_id,
            delta,
        }) => on_thread(
            thread_id,
            ThreadEvent::AgentDelta {
                turn: TurnId(turn_id),
                text: delta,
            },
        ),
        Ok(Notification::ReasoningSummaryTextDelta {
            thread_id,
            turn_id,
            delta,
        }) => on_thread(
            thread_id,
            ThreadEvent::ReasoningDelta {
                turn: TurnId(turn_id),
                text: delta,
            },
        ),
        Ok(Notification::ServerRequestResolved {
            thread_id,
            request_id,
        }) => match RequestId::read(&request_id) {
            Some(rpc_id) => Inbound::Resolved {
                thread: ThreadId(thread_id),
                rpc_id,
            },
            None => Inbound::Event(thread_unknown(raw)),
        },
        Ok(Notification::ItemStarted {
            item: ThreadItem::AgentMessage { .. } | ThreadItem::Reasoning { .. },
            ..
        })
        | Err(_) => Inbound::Event(thread_unknown(raw)),
        Ok(Notification::ItemStarted {
            thread_id,
            turn_id,
            item,
        }) => item_event(thread_id, turn_id, ItemPhase::Started, item),
        Ok(Notification::ItemCompleted {
            thread_id,
            turn_id,
            item,
        }) => item_event(thread_id, turn_id, ItemPhase::Completed, item),
    }
}

fn item_event(thread: String, turn: String, phase: ItemPhase, item: ThreadItem) -> Inbound {
    let turn = TurnId(turn);
    on_thread(
        thread,
        match item {
            ThreadItem::AgentMessage { text } => ThreadEvent::AgentMessage { turn, text },
            ThreadItem::Reasoning { summary } => ThreadEvent::Reasoning {
                turn,
                text: summary.join("\n\n"),
            },
            ThreadItem::CommandExecution {
                id,
                command,
                status,
            } => tool_event(turn, id, phase, ToolKind::Command { command }, status),
            ThreadItem::FileChange {
                id,
                changes,
                status,
            } => tool_event(
                turn,
                id,
                phase,
                ToolKind::FileChange {
                    files: changes
                        .into_iter()
                        .map(|file| FileChange {
                            path: file.path,
                            change: match file.kind {
                                ChangeKind::Add => Change::Write,
                                ChangeKind::Update => Change::Edit,
                                ChangeKind::Delete => Change::Delete,
                            },
                        })
                        .collect(),
                },
                status,
            ),
        },
    )
}

fn tool_event(
    turn: TurnId,
    id: String,
    phase: ItemPhase,
    tool: ToolKind,
    status: String,
) -> ThreadEvent {
    ThreadEvent::ToolCall {
        turn,
        call: ToolCallId(id),
        phase: match phase {
            ItemPhase::Started => ToolPhase::Started { tool },
            ItemPhase::Completed => ToolPhase::Finished {
                outcome: match status.as_str() {
                    "completed" => ToolOutcome::Succeeded,
                    "declined" => ToolOutcome::Declined,
                    _ => ToolOutcome::Failed,
                },
            },
        },
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

/// A request we originate.
#[derive(Debug, Clone)]
pub(crate) enum Call {
    Initialize,
    ModelList,
    ThreadStart(ThreadOptions),
    ThreadResume(ThreadId, ThreadOptions),
    ThreadRevert {
        thread: ThreadId,
        before: TurnId,
    },
    ThreadTurns {
        thread: ThreadId,
        cursor: Option<String>,
    },
    TurnStart {
        thread: ThreadId,
        text: String,
    },
    TurnInterrupt {
        thread: ThreadId,
        turn: TurnId,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InitializeParams {
    client_info: ClientInfo,
    capabilities: Capabilities,
}

#[derive(Serialize)]
struct ClientInfo {
    name: String,
    version: String,
}

/// `experimentalApi` is `true`, which brings `availableDecisions` on approvals.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Capabilities {
    experimental_api: bool,
}

/// `thread/start` and `thread/resume`; absent optional values are omitted.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    history_mode: Option<HistoryMode>,
    approval_policy: ApprovalPolicy,
    sandbox: Sandbox,
    cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    config: ThreadConfig,
}

/// Revert requires persisted paginated history; request it at thread creation.
#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum HistoryMode {
    Paginated,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadRevertParams {
    thread_id: String,
    before_turn_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadTurnsParams {
    thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
    limit: u32,
    sort_direction: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadTurnsPage {
    data: Vec<Id>,
    next_cursor: Value,
}

pub(crate) fn history_page(value: Value) -> serde_json::Result<(Vec<TurnId>, Option<String>)> {
    let page = serde_json::from_value::<ThreadTurnsPage>(value)?;
    // The field is required and nullable. Missing nextCursor must not look
    // like a complete history, as serde's missing Option field would.
    let cursor = serde_json::from_value::<Option<String>>(page.next_cursor)?;
    Ok((
        page.data.into_iter().map(|turn| TurnId(turn.id)).collect(),
        cursor,
    ))
}

/// The `config` overrides of every thread: the reasoning effort, if set;
/// reasoning summaries, which a custom model provider gets none of by
/// default (PROTOCOL.md); and the feature that lets the model ask the user
/// questions outside plan mode, which Codex 0.166 marks under development.
#[derive(Serialize)]
struct ThreadConfig {
    features: Features,
    model_reasoning_summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_reasoning_effort: Option<String>,
}

#[derive(Serialize)]
struct Features {
    default_mode_request_user_input: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnStartParams {
    thread_id: String,
    input: Vec<UserInput>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum UserInput {
    Text { text: String },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnInterruptParams {
    thread_id: String,
    turn_id: String,
}

impl Call {
    pub(crate) fn method(&self) -> &str {
        match self {
            Self::Initialize => INITIALIZE,
            Self::ModelList => "model/list",
            Self::ThreadStart(_) => "thread/start",
            Self::ThreadResume(_, _) => "thread/resume",
            Self::ThreadRevert { .. } => "thread/revert",
            Self::ThreadTurns { .. } => "thread/turns/list",
            Self::TurnStart { .. } => "turn/start",
            Self::TurnInterrupt { .. } => "turn/interrupt",
        }
    }

    fn params(&self) -> serde_json::Result<Value> {
        match self {
            Self::Initialize => serde_json::to_value(InitializeParams {
                client_info: ClientInfo {
                    name: String::from("conductor"),
                    version: String::from(env!("CARGO_PKG_VERSION")),
                },
                capabilities: Capabilities {
                    experimental_api: true,
                },
            }),
            Self::ModelList => Ok(Value::Object(serde_json::Map::new())),
            Self::ThreadStart(opts) => thread_params(None, Some(HistoryMode::Paginated), opts),
            Self::ThreadResume(thread, opts) => thread_params(Some(thread.0.clone()), None, opts),
            Self::ThreadRevert { thread, before } => serde_json::to_value(ThreadRevertParams {
                thread_id: thread.0.clone(),
                before_turn_id: before.0.clone(),
            }),
            Self::ThreadTurns { thread, cursor } => serde_json::to_value(ThreadTurnsParams {
                thread_id: thread.0.clone(),
                cursor: cursor.clone(),
                limit: 100,
                sort_direction: String::from("asc"),
            }),
            Self::TurnStart { thread, text } => serde_json::to_value(TurnStartParams {
                thread_id: thread.0.clone(),
                input: Vec::from([UserInput::Text { text: text.clone() }]),
            }),
            Self::TurnInterrupt { thread, turn } => serde_json::to_value(TurnInterruptParams {
                thread_id: thread.0.clone(),
                turn_id: turn.0.clone(),
            }),
        }
    }
}

fn thread_params(
    thread_id: Option<String>,
    history_mode: Option<HistoryMode>,
    opts: &ThreadOptions,
) -> serde_json::Result<Value> {
    serde_json::to_value(ThreadParams {
        thread_id,
        history_mode,
        approval_policy: opts.approval_policy,
        sandbox: opts.sandbox,
        cwd: opts.cwd.display().to_string(),
        model: opts.model.clone(),
        config: ThreadConfig {
            features: Features {
                default_mode_request_user_input: true,
            },
            model_reasoning_summary: String::from("auto"),
            model_reasoning_effort: opts.effort.map(|effort| String::from(effort.wire())),
        },
    })
}

#[derive(Deserialize)]
struct ModelList {
    data: Vec<Model>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Model {
    id: String,
    display_name: String,
    description: String,
    hidden: Option<bool>,
    is_default: Option<bool>,
    supported_reasoning_efforts: Vec<SupportedEffort>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SupportedEffort {
    reasoning_effort: String,
}

/// The models `model/list` offers, without the hidden ones.
pub(crate) fn models(result: Value) -> serde_json::Result<Models> {
    let list = serde_json::from_value::<ModelList>(result)?;
    let shown = list
        .data
        .into_iter()
        .filter(|m| m.hidden != Some(true))
        .collect::<Vec<Model>>();
    Ok(Models {
        default: shown
            .iter()
            .find(|m| m.is_default == Some(true))
            .map(|m| ModelId(m.id.clone())),
        choices: shown
            .into_iter()
            .map(|m| ModelChoice {
                id: ModelId(m.id),
                name: m.display_name,
                description: m.description,
                efforts: m
                    .supported_reasoning_efforts
                    .iter()
                    .filter_map(|e| Effort::parse(&e.reasoning_effort))
                    .collect(),
            })
            .collect(),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadResponse {
    thread: Id,
    model: String,
    reasoning_effort: Option<String>,
}

/// What `thread/start` and `thread/resume` say the thread runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadSettings {
    pub model: String,
    pub effort: Option<Effort>,
}

#[derive(Deserialize)]
struct TurnResponse {
    turn: Id,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitializeResponse {
    user_agent: String,
}

/// `thread/start` and `thread/resume` both answer `{thread: {id, ..},
/// model, reasoningEffort, ..}`.
pub(crate) fn thread_started(result: Value) -> serde_json::Result<(ThreadId, ThreadSettings)> {
    let r = serde_json::from_value::<ThreadResponse>(result)?;
    Ok((
        ThreadId(r.thread.id),
        ThreadSettings {
            model: r.model,
            effort: r.reasoning_effort.as_deref().and_then(Effort::parse),
        },
    ))
}

pub(crate) fn turn_id(result: Value) -> serde_json::Result<TurnId> {
    let r = serde_json::from_value::<TurnResponse>(result)?;
    Ok(TurnId(r.turn.id))
}

/// `userAgent` from `initialize`, such as `conductor/0.166.0 (Ubuntu ...)`.
pub(crate) fn user_agent(result: Value) -> serde_json::Result<String> {
    let r = serde_json::from_value::<InitializeResponse>(result)?;
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

/// Why a decision did not become an answer.
#[derive(Debug)]
pub(crate) enum AnswerError {
    /// The decision cannot answer a request of this shape.
    NotApplicable,
    /// The answer would not encode as JSON.
    Encode(serde_json::Error),
}

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

/// The answer to a command or file change approval.
#[derive(Serialize)]
struct PlainAnswer {
    decision: PlainDecision,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum PlainDecision {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
}

/// The JSON-RPC `result` that answers an open request with `decision`.
pub(crate) fn answer(shape: &AnswerShape, decision: Decision) -> Result<Value, AnswerError> {
    match (shape, decision) {
        (_, Decision::Raw(result)) => Ok(result),
        (AnswerShape::UserInput(ids), Decision::Answer(answers)) => encoded(UserInputAnswer {
            answers: ids
                .iter()
                .cloned()
                .zip(answers)
                .map(|(id, answer)| (id, QuestionAnswer { answers: answer.0 }))
                .collect(),
        }),
        (AnswerShape::UserInput(_), _)
        | (
            AnswerShape::Command | AnswerShape::FileChange | AnswerShape::Permissions(_),
            Decision::Answer(_),
        ) => Err(AnswerError::NotApplicable),
        (AnswerShape::Command | AnswerShape::FileChange, decision) => encoded(PlainAnswer {
            decision: plain(decision),
        }),
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

fn plain(decision: Decision) -> PlainDecision {
    match decision {
        Decision::Accept => PlainDecision::Accept,
        Decision::AcceptForSession => PlainDecision::AcceptForSession,
        Decision::Decline => PlainDecision::Decline,
        Decision::Cancel | Decision::Raw(_) | Decision::Answer(_) => PlainDecision::Cancel,
    }
}

/// The answer to a user-input request: each question's id to its answers.
#[derive(Serialize)]
struct UserInputAnswer {
    answers: BTreeMap<String, QuestionAnswer>,
}

#[derive(Serialize)]
struct QuestionAnswer {
    answers: Vec<String>,
}

/// Declining a permissions request grants an empty profile.
#[derive(Serialize)]
struct NoPermissions {
    permissions: Value,
}

fn encoded<T: Serialize>(answer: T) -> Result<Value, AnswerError> {
    serde_json::to_value(answer).map_err(AnswerError::Encode)
}

/// JSON-RPC "method not found".
const METHOD_NOT_FOUND: i64 = 32_601_i64.wrapping_neg();

// Fields are in alphabetical order, the order `serde_json::json!` wrote them in.
#[derive(Serialize)]
#[serde(untagged)]
enum OutboundFrame {
    Request {
        id: RequestId,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
    },
    Answer {
        id: RequestId,
        result: Value,
    },
    Reject {
        error: RpcError,
        id: RequestId,
    },
}

#[derive(Serialize, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

pub(crate) fn encode(msg: Outbound) -> serde_json::Result<String> {
    serde_json::to_string(&match msg {
        Outbound::Request { id, call } => OutboundFrame::Request {
            id: RequestId::Integer(id),
            method: String::from(call.method()),
            params: call.params()?,
        },
        Outbound::Initialized => OutboundFrame::Notification {
            method: String::from("initialized"),
        },
        Outbound::Answer { rpc_id, result } => OutboundFrame::Answer { id: rpc_id, result },
        Outbound::Reject { rpc_id, message } => OutboundFrame::Reject {
            error: RpcError {
                code: METHOD_NOT_FOUND,
                message,
            },
            id: rpc_id,
        },
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use proto::{Answer, Effort, Question, QuestionOption, Selection};
    use serde_json::Value;

    use super::{
        AnswerError, AnswerShape, ApprovalId, ApprovalKind, ApprovalPolicy, Call, CodexEvent,
        Decision, Inbound, Outbound, PermissionAsk, RequestId, Sandbox, ThreadEvent, ThreadId,
        ThreadOptions, TurnId, answer, encode, encoded, parse_line,
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
            effort: Some(Effort::Low),
            approval_policy: ApprovalPolicy::OnRequest,
            sandbox: Sandbox::ReadOnly,
        }
    }

    #[test]
    fn history_reads_native_turns_and_cursor_only() {
        assert_eq!(super::history_page(json(r#"{"data":[{"id":"tu_1","status":"completed","items":[]},{"id":"tu_2","status":"interrupted"}],"nextCursor":"page-2","backwardsCursor":"unused"}"#)).unwrap(),
            (Vec::from([TurnId(String::from("tu_1")),TurnId(String::from("tu_2"))]),Some(String::from("page-2"))));
        assert_eq!(
            request(Call::ThreadTurns {
                thread: ThreadId(String::from("th_1")),
                cursor: Some(String::from("page-2"))
            }),
            json(
                r#"{"id":7,"method":"thread/turns/list","params":{"threadId":"th_1","cursor":"page-2","limit":100,"sortDirection":"asc"}}"#
            )
        );
    }

    #[test]
    fn incomplete_or_malformed_history_is_not_an_empty_prefix() {
        let pages = Vec::from([
            r#"{"data":[]}"#,
            r#"{"data":[],"nextCursor":42}"#,
            r#"{"data":[{"status":"completed"}],"nextCursor":null}"#,
            r#"{"data":null,"nextCursor":null}"#,
        ]);
        assert_eq!(
            pages
                .into_iter()
                .all(|page| super::history_page(json(page)).is_err())
                .then_some(()),
            Some(())
        );
    }

    #[test]
    fn revert_excludes_the_selected_native_turn() {
        assert_eq!(
            request(Call::ThreadRevert {
                thread: ThreadId(String::from("th_1")),
                before: TurnId(String::from("tu_2")),
            }),
            json(
                r#"{"id":7,"method":"thread/revert","params":{"threadId":"th_1","beforeTurnId":"tu_2"}}"#
            )
        );
    }

    #[test]
    fn request_ids_accept_only_integer_and_string_shapes() {
        let cases = Vec::from([
            ("0", Some(RequestId::Integer(0))),
            ("-9", Some(RequestId::Integer(9_i64.wrapping_neg()))),
            (r#""rpc-1""#, Some(RequestId::String(String::from("rpc-1")))),
            ("1.0", None),
            ("9223372036854775808", None),
            ("true", None),
            ("null", None),
            ("{}", None),
        ]);
        assert_eq!(
            cases
                .iter()
                .map(|(wire, _)| RequestId::read(&json(wire)))
                .collect::<Vec<Option<RequestId>>>(),
            cases
                .into_iter()
                .map(|(_, expected)| expected)
                .collect::<Vec<Option<RequestId>>>()
        );
    }

    #[test]
    fn replies_keep_a_null_result_and_a_string_id() {
        assert_eq!(
            encode(Outbound::Answer {
                rpc_id: RequestId::String(String::from("rpc-1")),
                result: Value::Null,
            })
            .unwrap(),
            r#"{"id":"rpc-1","result":null}"#
        );
    }

    #[test]
    fn thread_start_omits_absent_model_thread_and_effort() {
        let options = ThreadOptions {
            model: None,
            effort: None,
            ..opts()
        };
        assert_eq!(
            request(Call::ThreadStart(options)),
            json(
                r#"{"id":7,"method":"thread/start","params":{"historyMode":"paginated",
                "approvalPolicy":"on-request","sandbox":"read-only","cwd":"/tmp/s2-scratch",
                "config":{"features":{"default_mode_request_user_input":true},
                "model_reasoning_summary":"auto"}}}"#
            )
        );
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
                r#"{"id": 7, "method": "thread/start", "params": {"historyMode":"paginated",
                "approvalPolicy": "on-request", "sandbox": "read-only", "cwd": "/tmp/s2-scratch",
                "model": "gpt-6-luna", "config": {"model_reasoning_effort": "low",
                "model_reasoning_summary": "auto", "features": {"default_mode_request_user_input": true}}}}"#
            )
        );
        assert_eq!(
            request(Call::ThreadResume(ThreadId(String::from("th_1")), opts())),
            json(
                r#"{"id": 7, "method": "thread/resume", "params": {
                "threadId": "th_1", "approvalPolicy": "on-request", "sandbox": "read-only",
                "cwd": "/tmp/s2-scratch", "model": "gpt-6-luna",
                "config": {"model_reasoning_effort": "low",
                "model_reasoning_summary": "auto", "features": {"default_mode_request_user_input": true}}}}"#
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

    /// Recorded from codex-cli 0.166.0 (probe, 2026-11-12).
    const USER_INPUT: &str = r#"{"method": "item/tool/requestUserInput", "id": 0, "params": {"threadId": "th_1", "turnId": "tu_1", "itemId": "call_1", "questions": [{"id": "preferred_color", "header": "Color", "question": "Which color do you prefer?", "isOther": true, "isSecret": false, "options": [{"label": "Red", "description": "Choose Red."}, {"label": "Blue", "description": "Choose Blue."}]}], "isBlocking": false, "autoResolutionMs": null}}"#;

    #[test]
    fn a_secret_question_is_declined_without_its_content() {
        let line = USER_INPUT.replace(r#""isSecret": false"#, r#""isSecret": true"#);
        assert_eq!(
            parse_line(&line).unwrap(),
            Inbound::Declined {
                rpc_id: RequestId::Integer(0),
                event: CodexEvent::Thread {
                    thread: ThreadId(String::from("th_1")),
                    event: ThreadEvent::SecretQuestionDeclined {
                        turn: TurnId(String::from("tu_1")),
                    },
                },
            }
        );
    }

    #[test]
    fn user_input_is_a_question_answered_by_question_id() {
        let (shape, event) = match parse_line(USER_INPUT).unwrap() {
            Inbound::Approval { shape, event, .. } => Some((shape, event)),
            Inbound::Event(_)
            | Inbound::Response { .. }
            | Inbound::Resolved { .. }
            | Inbound::Unsupported { .. }
            | Inbound::Declined { .. } => None,
        }
        .unwrap();
        assert_eq!(
            event,
            ThreadEvent::ApprovalRequested {
                id: ApprovalId(String::from("0")),
                turn: TurnId(String::from("tu_1")),
                item: String::from("call_1"),
                kind: ApprovalKind::UserInput {
                    questions: Vec::from([Question {
                        header: String::from("Color"),
                        text: String::from("Which color do you prefer?"),
                        options: Vec::from([
                            QuestionOption {
                                label: String::from("Red"),
                                description: String::from("Choose Red."),
                            },
                            QuestionOption {
                                label: String::from("Blue"),
                                description: String::from("Choose Blue."),
                            },
                        ]),
                        selection: Selection::One,
                    }]),
                },
                options: Vec::new(),
            }
        );
        assert_eq!(
            answered(
                &shape,
                Decision::Answer(Vec::from([Answer(Vec::from([String::from("Blue")]))]))
            ),
            json(r#"{"id": 0, "result": {"answers": {"preferred_color": {"answers": ["Blue"]}}}}"#)
        );
        assert_eq!(
            match answer(&shape, Decision::Accept) {
                Err(AnswerError::NotApplicable) => "not applicable",
                Err(AnswerError::Encode(_)) => "encode",
                Ok(_) => "answered",
            },
            "not applicable"
        );
    }

    #[test]
    fn an_answer_that_does_not_encode_is_an_encode_error() {
        assert_eq!(
            match encoded(BTreeMap::from([((1, 2), "a tuple is not a JSON key")])) {
                Err(AnswerError::Encode(e)) => e.to_string(),
                Err(AnswerError::NotApplicable) => String::from("not applicable"),
                Ok(value) => value.to_string(),
            },
            "key must be a string"
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
                    item: String::from("exec-1"),
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
    fn a_permissions_request_lists_what_it_asks_for() {
        let line = r#"{"method":"item/permissions/requestApproval","id":3,"params":{"threadId":"th_1","turnId":"tu_1","itemId":"exec-2","startedAtMs":1,"cwd":"/w","reason":"to fetch","permissions":{"network":{"enabled":true},"fileSystem":{"write":["/home/u/.cargo"],"entries":[{"access":"write","path":{"type":"special","value":{"kind":"slashTmp"}}},{"access":"read","path":{"type":"path","path":"/etc/hosts"}}]}}}}"#;
        assert_eq!(
            match parse_line(line).unwrap() {
                Inbound::Approval {
                    event:
                        ThreadEvent::ApprovalRequested {
                            kind: ApprovalKind::Permissions { reason, asks },
                            ..
                        },
                    ..
                } => Some((reason, asks)),
                _ => None,
            },
            Some((
                Some(String::from("to fetch")),
                Vec::from([
                    PermissionAsk::Network,
                    PermissionAsk::Read(Vec::from([String::from("/etc/hosts")])),
                    PermissionAsk::Write(Vec::from([
                        String::from("/home/u/.cargo"),
                        String::from("/tmp"),
                    ])),
                ])
            ))
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

    #[test]
    fn tool_notifications_emit_shared_phases_and_file_changes() {
        use proto::{Change, FileChange, ToolCallId, ToolKind, ToolOutcome, ToolPhase};
        let cases = Vec::from([
            (
                r#"{"method":"item/started","params":{"threadId":"th","turnId":"tu","item":{"type":"commandExecution","id":"call","command":"pwd","status":"inProgress"}}}"#,
                ToolPhase::Started {
                    tool: ToolKind::Command {
                        command: String::from("pwd"),
                    },
                },
            ),
            (
                r#"{"method":"item/started","params":{"threadId":"th","turnId":"tu","item":{"type":"fileChange","id":"call","changes":[{"path":"a","kind":{"type":"add"}},{"path":"b","kind":{"type":"update","move_path":"elsewhere"}},{"path":"c","kind":{"type":"delete"}}],"status":"inProgress"}}}"#,
                ToolPhase::Started {
                    tool: ToolKind::FileChange {
                        files: Vec::from([
                            FileChange {
                                path: String::from("a"),
                                change: Change::Write,
                            },
                            FileChange {
                                path: String::from("b"),
                                change: Change::Edit,
                            },
                            FileChange {
                                path: String::from("c"),
                                change: Change::Delete,
                            },
                        ]),
                    },
                },
            ),
            (
                r#"{"method":"item/completed","params":{"threadId":"th","turnId":"tu","item":{"type":"commandExecution","id":"call","command":"pwd","status":"completed"}}}"#,
                ToolPhase::Finished {
                    outcome: ToolOutcome::Succeeded,
                },
            ),
            (
                r#"{"method":"item/completed","params":{"threadId":"th","turnId":"tu","item":{"type":"commandExecution","id":"call","command":"pwd","status":"declined"}}}"#,
                ToolPhase::Finished {
                    outcome: ToolOutcome::Declined,
                },
            ),
            (
                r#"{"method":"item/completed","params":{"threadId":"th","turnId":"tu","item":{"type":"commandExecution","id":"call","command":"pwd","status":"failed"}}}"#,
                ToolPhase::Finished {
                    outcome: ToolOutcome::Failed,
                },
            ),
            (
                r#"{"method":"item/completed","params":{"threadId":"th","turnId":"tu","item":{"type":"fileChange","id":"call","changes":[],"status":"new_status"}}}"#,
                ToolPhase::Finished {
                    outcome: ToolOutcome::Failed,
                },
            ),
        ]);
        let (got, want) = cases
            .into_iter()
            .map(|(line, phase)| {
                (
                    parse_line(line).unwrap(),
                    Inbound::Event(CodexEvent::Thread {
                        thread: super::ThreadId(String::from("th")),
                        event: super::ThreadEvent::ToolCall {
                            turn: super::TurnId(String::from("tu")),
                            call: ToolCallId(String::from("call")),
                            phase,
                        },
                    }),
                )
            })
            .unzip::<Inbound, Inbound, Vec<Inbound>, Vec<Inbound>>();
        assert_eq!(got, want);
    }

    #[test]
    fn completed_turns_emit_shared_outcomes_with_failure_messages() {
        use proto::TurnOutcome;
        let cases = Vec::from([
            (
                r#"{"method":"turn/completed","params":{"threadId":"th","turn":{"id":"tu","status":"completed"}}}"#,
                TurnOutcome::Completed,
            ),
            (
                r#"{"method":"turn/completed","params":{"threadId":"th","turn":{"id":"tu","status":"interrupted"}}}"#,
                TurnOutcome::Interrupted,
            ),
            (
                r#"{"method":"turn/completed","params":{"threadId":"th","turn":{"id":"tu","status":"failed","error":{"message":"unavailable"}}}}"#,
                TurnOutcome::Failed {
                    message: String::from("unavailable"),
                },
            ),
            (
                r#"{"method":"turn/completed","params":{"threadId":"th","turn":{"id":"tu","status":"failed"}}}"#,
                TurnOutcome::Failed {
                    message: String::from("the turn failed"),
                },
            ),
            (
                r#"{"method":"turn/completed","params":{"threadId":"th","turn":{"id":"tu","status":"inProgress"}}}"#,
                TurnOutcome::Failed {
                    message: String::from("the turn failed"),
                },
            ),
        ]);
        let (got, want) = cases
            .into_iter()
            .map(|(line, outcome)| {
                (
                    parse_line(line).unwrap(),
                    Inbound::Event(CodexEvent::Thread {
                        thread: super::ThreadId(String::from("th")),
                        event: super::ThreadEvent::TurnCompleted {
                            turn: super::TurnId(String::from("tu")),
                            outcome,
                        },
                    }),
                )
            })
            .unzip::<Inbound, Inbound, Vec<Inbound>, Vec<Inbound>>();
        assert_eq!(got, want);
    }
}
