//! The stdio boundary. Each app-server stdout line is parsed into the few
//! JSON-RPC messages we read and mapped into our own types here, and every line
//! we write is built here. Methods and fields we do not model are ignored or
//! passed on as `Unknown`.

use std::path::PathBuf;

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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum RequestId {
    Integer(i64),
    String(String),
}

impl RequestId {
    fn read(id: &Value) -> Option<RequestId> {
        match id {
            Value::Number(n) => n.as_i64().map(RequestId::Integer),
            Value::String(s) => Some(RequestId::String(s.clone())),
            _ => None,
        }
    }

    fn wire(self) -> Value {
        match self {
            RequestId::Integer(n) => Value::from(n),
            RequestId::String(s) => Value::from(s),
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ChangeKind {
    Add,
    Delete,
    Update,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
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
    Item {
        turn: TurnId,
        phase: ItemPhase,
        item: Item,
    },
    ApprovalRequested {
        id: ApprovalId,
        turn: TurnId,
        /// The item the request is about, as in its `Item` event.
        item: String,
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
    UserInput { questions: Value },
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
    CommandExecution {
        id: String,
        command: String,
        status: String,
        #[serde(rename = "exitCode")]
        exit_code: Option<i64>,
    },
    FileChange {
        id: String,
        changes: Vec<FileChange>,
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
            shape: AnswerShape::UserInput,
            kind: ApprovalKind::UserInput { questions },
            options: Vec::new(),
        },
    })
}

/// A request whose method we don't serve, or whose params don't fit, still
/// gets an error reply.
fn request(rpc_id: RequestId, method: String, params: Option<Value>, raw: Value) -> Inbound {
    let served = serde_json::from_value::<ServerRequest>(raw.clone())
        .and_then(asked)
        .and_then(|asked| {
            serde_json::from_value::<Target>(params.unwrap_or(Value::Null))
                .map(|target| (asked, target))
        });
    match served {
        Ok((asked, target)) => Inbound::Approval {
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
                    message: turn.error.map(|e| e.message),
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
            item: ThreadItem::AgentMessage { .. },
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
            ThreadItem::CommandExecution {
                id,
                command,
                status,
                exit_code,
            } => ThreadEvent::Item {
                turn,
                phase,
                item: Item::Command {
                    id,
                    command,
                    status,
                    exit_code,
                },
            },
            ThreadItem::FileChange {
                id,
                changes,
                status,
            } => ThreadEvent::Item {
                turn,
                phase,
                item: Item::FileChange {
                    id,
                    changes,
                    status,
                },
            },
        },
    )
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
    TurnStart { thread: ThreadId, text: String },
    TurnInterrupt { thread: ThreadId, turn: TurnId },
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
    experimental_api: Value,
}

/// `thread/start` and `thread/resume`; a missing value goes out as `null`,
/// which `params` then drops.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadParams {
    thread_id: Option<String>,
    approval_policy: ApprovalPolicy,
    sandbox: Sandbox,
    cwd: String,
    model: Option<String>,
    config: Option<EffortConfig>,
}

/// The `config` override that sets the reasoning effort.
#[derive(Serialize)]
struct EffortConfig {
    model_reasoning_effort: String,
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
                    experimental_api: Value::Bool(true),
                },
            }),
            Self::ModelList => Ok(Value::Object(serde_json::Map::new())),
            Self::ThreadStart(opts) => thread_params(None, opts),
            Self::ThreadResume(thread, opts) => thread_params(Some(thread.0.clone()), opts),
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

fn thread_params(thread_id: Option<String>, opts: &ThreadOptions) -> serde_json::Result<Value> {
    let params = serde_json::to_value(ThreadParams {
        thread_id,
        approval_policy: opts.approval_policy,
        sandbox: opts.sandbox,
        cwd: opts.cwd.display().to_string(),
        model: opts.model.clone(),
        config: opts.effort.clone().map(|effort| EffortConfig {
            model_reasoning_effort: effort,
        }),
    })?;
    Ok(match params {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .filter(|(_, value)| !value.is_null())
                .collect(),
        ),
        other => other,
    })
}

#[derive(Deserialize)]
struct ModelList {
    data: Vec<Model>,
}

#[derive(Deserialize)]
struct Model {
    id: String,
    description: String,
}

pub(crate) fn models(result: Value) -> serde_json::Result<Vec<ModelInfo>> {
    let list = serde_json::from_value::<ModelList>(result)?;
    Ok(list
        .data
        .into_iter()
        .map(|m| ModelInfo {
            id: m.id,
            description: m.description,
        })
        .collect())
}

#[derive(Deserialize)]
struct ThreadResponse {
    thread: Id,
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

pub(crate) fn thread_id(result: Value) -> serde_json::Result<ThreadId> {
    // `thread/start` and `thread/resume` both answer `{thread: {id, ..}, ..}`.
    let r = serde_json::from_value::<ThreadResponse>(result)?;
    Ok(ThreadId(r.thread.id))
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
        (AnswerShape::UserInput, _) => Err(AnswerError::NotApplicable),
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
        Decision::Cancel | Decision::Raw(_) => PlainDecision::Cancel,
    }
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
    id: Value,
    result: Value,
}

#[derive(Serialize)]
struct RejectFrame {
    error: RpcError,
    id: Value,
}

#[derive(Serialize, Deserialize)]
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
            method: String::from("initialized"),
        }),
        Outbound::Answer { rpc_id, result } => serde_json::to_string(&AnswerFrame {
            id: rpc_id.wire(),
            result,
        }),
        Outbound::Reject { rpc_id, message } => serde_json::to_string(&RejectFrame {
            error: RpcError {
                code: METHOD_NOT_FOUND,
                message,
            },
            id: rpc_id.wire(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

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
            match answer(&AnswerShape::UserInput, Decision::Accept) {
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
}
