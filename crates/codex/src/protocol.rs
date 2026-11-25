use alloc::collections::BTreeSet;

use agents::Exit;
use agents::{
    Answer, Change, ChecklistEntry, ChecklistStatus, Effort, FileChange,
    ModelChoice, ModelId, Models, Question, QuestionOption, Selection,
    ToolCallId, ToolKind, ToolOutcome, ToolPhase, TurnOutcome,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::prelude::*;

pub const INITIALIZE: &str = "initialize";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThreadId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TurnId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ApprovalId(pub String);

impl ApprovalId {
    #[must_use]
    pub fn of(id: &RequestId) -> Self {
        Self(match id {
            RequestId::Integer(n) => format!("integer:{n}"),
            RequestId::String(s) => format!("string:{s}"),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Integer(i64),
    String(String),
}

impl RequestId {
    fn read(id: &Value) -> Option<Self> {
        serde_json::from_value(id.clone()).ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalPolicy {
    Untrusted,
    OnRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Sandbox {
    ReadOnly,
    WorkspaceWrite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadOptions {
    pub cwd: String,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub approval_policy: ApprovalPolicy,
    pub sandbox: Sandbox,
    pub instructions: Option<String>,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionAsk {
    Network,
    Read(Vec<String>),
    Write(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalKind {
    Command {
        command: Option<String>,
        cwd: Option<String>,
        reason: Option<String>,
    },
    FileChange {
        reason: Option<String>,
    },
    Permissions {
        reason: Option<String>,
        asks: Vec<PermissionAsk>,
    },
    UserInput {
        questions: Vec<Question>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildEvent {
    ThreadStarted {
        parent: ThreadId,
        name: Option<String>,
        kind: Option<String>,
    },
    Collaboration {
        spawn: Option<SpawnCall>,
        states: Vec<(ThreadId, ChildState)>,
    },
    Activity {
        call: ToolCallId,
        agent: ThreadId,
        path: String,
        activity: ChildActivity,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnCall {
    pub call: ToolCallId,
    pub description: Option<String>,
    pub agents: Vec<ThreadId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ChildState {
    pub status: ChildStatus,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChildStatus {
    PendingInit,
    Running,
    Interrupted,
    Completed,
    Errored,
    Shutdown,
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChildActivity {
    Started,
    Interacted,
    Interrupted,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadEvent {
    Started,
    Child(ChildEvent),
    TurnStarted {
        turn: TurnId,
    },
    TurnCompleted {
        turn: TurnId,
        outcome: TurnOutcome,
    },
    AgentDelta {
        turn: TurnId,
        text: String,
    },
    AgentMessage {
        turn: TurnId,
        text: String,
    },
    ReasoningDelta {
        turn: TurnId,
        text: String,
    },
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
        item: String,
        kind: ApprovalKind,
    },
    SecretQuestionDeclined {
        turn: TurnId,
    },
    ApprovalCancelled {
        id: ApprovalId,
    },
    PlanUpdated {
        turn: TurnId,
        steps: Vec<ChecklistEntry>,
    },
    SettingsChanged(ThreadSettings),
    Unknown(Value),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexEvent {
    Thread {
        thread: ThreadId,
        event: ThreadEvent,
    },
    Exited {
        status: Exit,
    },
    Unknown(Value),
}

impl CodexEvent {
    #[must_use]
    pub const fn diagnostic(&self) -> (&'static str, Option<&ThreadId>) {
        match self {
            Self::Thread { thread, event } => (
                match event {
                    ThreadEvent::Started => "thread_started",
                    ThreadEvent::Child(_) => "subagent",
                    ThreadEvent::TurnStarted { .. } => "turn_started",
                    ThreadEvent::TurnCompleted { .. } => "turn_completed",
                    ThreadEvent::AgentDelta { .. } => "agent_delta",
                    ThreadEvent::AgentMessage { .. } => "agent_message",
                    ThreadEvent::ReasoningDelta { .. } => "reasoning_delta",
                    ThreadEvent::Reasoning { .. } => "reasoning",
                    ThreadEvent::ToolCall { .. } => "tool_call",
                    ThreadEvent::ApprovalRequested { .. } => {
                        "approval_requested"
                    }
                    ThreadEvent::SecretQuestionDeclined { .. } => {
                        "secret_question_declined"
                    }
                    ThreadEvent::ApprovalCancelled { .. } => {
                        "approval_cancelled"
                    }
                    ThreadEvent::PlanUpdated { .. } => "plan_updated",
                    ThreadEvent::SettingsChanged(_) => {
                        "thread_settings_changed"
                    }
                    ThreadEvent::Unknown(_) => "unknown",
                },
                Some(thread),
            ),
            Self::Exited { .. } => ("exited", None),
            Self::Unknown(_) => ("unknown", None),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Accept,
    AcceptForSession,
    Decline,
    Answer(Vec<Answer>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerShape {
    Command,
    FileChange,
    Permissions(Value),
    UserInput(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inbound {
    Event(CodexEvent),
    Response {
        id: RequestId,
        outcome: Result<Value, ResponseError>,
    },
    Approval {
        rpc_id: RequestId,
        thread: ThreadId,
        shape: AnswerShape,
        event: ThreadEvent,
    },
    Resolved {
        thread: ThreadId,
        rpc_id: RequestId,
    },
    Unsupported {
        rpc_id: RequestId,
        method: String,
        event: CodexEvent,
    },
    Declined {
        rpc_id: RequestId,
        event: CodexEvent,
    },
}

#[derive(Deserialize)]
#[serde(tag = "method", content = "params")]
enum ServerRequest {
    #[serde(rename = "item/commandExecution/requestApproval")]
    Command {
        command: Option<String>,
        cwd: Option<String>,
        reason: Option<String>,
    },
    #[serde(rename = "item/fileChange/requestApproval")]
    FileChange { reason: Option<String> },
    #[serde(rename = "item/permissions/requestApproval")]
    Permissions {
        reason: Option<String>,
        permissions: PermissionProfile,
    },
    #[serde(rename = "item/tool/requestUserInput")]
    UserInput { questions: Vec<WireQuestion> },
}

#[derive(Deserialize)]
struct WireQuestion {
    id: String,
    header: String,
    question: String,
    options: Option<Vec<QuestionOption>>,
    #[serde(rename = "isSecret")]
    is_secret: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Target {
    #[serde(rename = "threadId")]
    thread: String,
    #[serde(rename = "turnId")]
    turn: String,
    #[serde(rename = "itemId")]
    item: String,
}

#[derive(Deserialize)]
#[serde(tag = "method", content = "params")]
enum Notification {
    #[serde(rename = "thread/started")]
    ThreadStarted { thread: StartedThread },
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
    #[serde(rename = "turn/plan/updated")]
    PlanUpdated {
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "turnId")]
        turn_id: String,
        plan: Vec<WirePlanStep>,
    },
    #[serde(rename = "thread/settings/updated")]
    SettingsUpdated {
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "threadSettings")]
        settings: WireSettings,
    },
}

#[derive(Deserialize)]
struct WirePlanStep {
    step: String,
    status: WirePlanStatus,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum WirePlanStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Deserialize)]
struct WireSettings {
    model: String,
    effort: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartedThread {
    id: String,
    source: Option<Value>,
    agent_nickname: Option<String>,
    agent_role: Option<String>,
}

#[derive(Deserialize)]
struct ThreadSpawn {
    parent_thread_id: String,
    agent_nickname: Option<String>,
    agent_role: Option<String>,
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
    #[serde(rename_all = "camelCase")]
    CollabAgentToolCall {
        id: String,
        tool: String,
        sender_thread_id: String,
        receiver_thread_ids: Vec<String>,
        prompt: Option<String>,
        agents_states: alloc::collections::BTreeMap<String, ChildState>,
    },
    #[serde(rename_all = "camelCase")]
    SubAgentActivity {
        id: String,
        agent_thread_id: String,
        agent_path: String,
        kind: ChildActivity,
    },
}

#[derive(Serialize, Deserialize)]
struct PermissionProfile {
    #[serde(skip_serializing_if = "Option::is_none")]
    network: Option<Network>,
    #[serde(rename = "fileSystem")]
    #[serde(skip_serializing_if = "Option::is_none")]
    file_system: Option<FileSystem>,
}

#[derive(Serialize, Deserialize)]
struct Network {
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
}

#[derive(Serialize, Deserialize)]
struct FileSystem {
    #[serde(skip_serializing_if = "Option::is_none")]
    read: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    write: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    entries: Option<Vec<FileSystemEntry>>,
}

#[derive(Serialize, Deserialize)]
struct FileSystemEntry {
    access: Access,
    path: Place,
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Access {
    Read,
    Write,
    Deny,
}

#[derive(Serialize, Deserialize)]
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

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum SpecialPlace {
    Root,
    Minimal,
    ProjectRoots {
        #[serde(skip_serializing_if = "Option::is_none")]
        subpath: Option<String>,
    },
    Tmpdir,
    SlashTmp,
    Unknown {
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        subpath: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseError {
    Rejected(String),
    Malformed(&'static str),
}

pub fn parse_line(line: &str) -> serde_json::Result<Inbound> {
    let raw = serde_json::from_str::<Value>(line)?;
    let rpc_id = raw.get("id").and_then(RequestId::read);
    let rpc_method = raw.get("method").and_then(Value::as_str);
    Ok(match (rpc_id, rpc_method) {
        (Some(id), Some(method))
            if raw.get("result").is_none() && raw.get("error").is_none() =>
        {
            request(id, method.to_owned(), raw.get("params").cloned(), raw)
        }
        (Some(id), _) => Inbound::Response {
            id,
            outcome: response(&raw),
        },
        (None, Some(_)) => notification(raw),
        (None, None) => Inbound::Event(thread_unknown(raw)),
    })
}

fn response(raw: &Value) -> Result<Value, ResponseError> {
    match (raw.get("method"), raw.get("result"), raw.get("error")) {
        (None, Some(result), None) => Ok(result.clone()),
        (None, None, Some(error)) => {
            Err(serde_json::from_value::<RpcError>(error.clone()).map_or(
                ResponseError::Malformed("malformed JSON-RPC error response"),
                |decoded| ResponseError::Rejected(decoded.message),
            ))
        }
        _ => Err(ResponseError::Malformed("malformed JSON-RPC response")),
    }
}

enum Served {
    Asked(Asked),
    Secret,
}

struct Asked {
    shape: AnswerShape,
    kind: ApprovalKind,
}

fn permission_asks(profile: &PermissionProfile) -> Vec<PermissionAsk> {
    let network = profile
        .network
        .as_ref()
        .and_then(|network| network.enabled)
        .and_then(|enabled| enabled.then_some(PermissionAsk::Network));
    let (read, write) = profile.file_system.as_ref().map_or_else(
        || (Vec::new(), Vec::new()),
        |file_system| {
            (
                listed(
                    file_system.read.as_deref(),
                    file_system.entries.as_deref(),
                    &Access::Read,
                ),
                listed(
                    file_system.write.as_deref(),
                    file_system.entries.as_deref(),
                    &Access::Write,
                ),
            )
        },
    );
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

fn listed(
    plain: Option<&[String]>,
    entries: Option<&[FileSystemEntry]>,
    access: &Access,
) -> Vec<String> {
    plain
        .into_iter()
        .flatten()
        .cloned()
        .chain(
            entries
                .into_iter()
                .flatten()
                .filter(|entry| &entry.access == access)
                .map(|entry| place(&entry.path)),
        )
        .collect()
}

fn place(target: &Place) -> String {
    match target {
        Place::Path { path } => path.clone(),
        Place::GlobPattern { pattern } => pattern.clone(),
        Place::Special { value } => match value {
            SpecialPlace::Root => String::from("/"),
            SpecialPlace::Minimal => String::from("the minimal system paths"),
            SpecialPlace::ProjectRoots { subpath: None } => {
                String::from("the project roots")
            }
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
        ServerRequest::UserInput { questions } => {
            if questions
                .iter()
                .any(|question| question.is_secret == Some(true))
            {
                return Ok(Served::Secret);
            }
            let keys = questions
                .iter()
                .map(|one| &one.id)
                .collect::<BTreeSet<&String>>();
            if keys.len() != questions.len() {
                return Err(serde::de::Error::custom("duplicate question ID"));
            }
            asked(ServerRequest::UserInput { questions }).map(Served::Asked)
        }
        other => asked(other).map(Served::Asked),
    }
}

fn asked(request: ServerRequest) -> serde_json::Result<Asked> {
    Ok(match request {
        ServerRequest::Command {
            command,
            cwd,
            reason,
        } => Asked {
            shape: AnswerShape::Command,
            kind: ApprovalKind::Command {
                command,
                cwd,
                reason,
            },
        },
        ServerRequest::FileChange { reason } => Asked {
            shape: AnswerShape::FileChange,
            kind: ApprovalKind::FileChange { reason },
        },
        ServerRequest::Permissions {
            reason,
            permissions,
        } => Asked {
            kind: ApprovalKind::Permissions {
                reason,
                asks: permission_asks(&permissions),
            },
            shape: AnswerShape::Permissions(serde_json::to_value(permissions)?),
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
        },
    })
}

fn request(
    rpc_id: RequestId,
    method: String,
    params: Option<Value>,
    raw: Value,
) -> Inbound {
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
                thread: ThreadId(target.thread),
                event: ThreadEvent::SecretQuestionDeclined {
                    turn: TurnId(target.turn),
                },
            },
        },
        Ok((Served::Asked(asked), target)) => Inbound::Approval {
            thread: ThreadId(target.thread),
            shape: asked.shape,
            event: ThreadEvent::ApprovalRequested {
                id: ApprovalId::of(&rpc_id),
                turn: TurnId(target.turn),
                item: target.item,
                kind: asked.kind,
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

const fn on_thread(thread: String, event: ThreadEvent) -> Inbound {
    Inbound::Event(CodexEvent::Thread {
        thread: ThreadId(thread),
        event,
    })
}

fn notification(raw: Value) -> Inbound {
    match serde_json::from_value::<Notification>(raw.clone()) {
        Ok(Notification::ThreadStarted { thread }) => {
            let event = thread
                .source
                .and_then(|source| {
                    serde_json::from_value::<ThreadSpawn>(
                        source.pointer("/subAgent/thread_spawn")?.clone(),
                    )
                    .ok()
                })
                .map_or(ThreadEvent::Started, |spawn| {
                    ThreadEvent::Child(ChildEvent::ThreadStarted {
                        parent: ThreadId(spawn.parent_thread_id),
                        name: spawn.agent_nickname.or(thread.agent_nickname),
                        kind: spawn.agent_role.or(thread.agent_role),
                    })
                });
            on_thread(thread.id, event)
        }
        Ok(Notification::TurnStarted { thread_id, turn }) => on_thread(
            thread_id,
            ThreadEvent::TurnStarted {
                turn: TurnId(turn.id),
            },
        ),
        Ok(Notification::TurnCompleted { thread_id, turn }) => {
            on_thread(thread_id, turn_completed(turn))
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
        }) => RequestId::read(&request_id).map_or_else(
            || Inbound::Event(thread_unknown(raw)),
            |rpc_id| Inbound::Resolved {
                thread: ThreadId(thread_id),
                rpc_id,
            },
        ),
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
        Ok(Notification::PlanUpdated {
            thread_id,
            turn_id,
            plan,
        }) => on_thread(
            thread_id,
            ThreadEvent::PlanUpdated {
                turn: TurnId(turn_id),
                steps: plan.into_iter().map(plan_step).collect(),
            },
        ),
        Ok(Notification::SettingsUpdated {
            thread_id,
            settings,
        }) => on_thread(
            thread_id,
            ThreadEvent::SettingsChanged(ThreadSettings {
                model: settings.model,
                effort: settings.effort.as_deref().and_then(Effort::parse),
            }),
        ),
    }
}

fn turn_completed(turn: Turn) -> ThreadEvent {
    let outcome = match turn.status {
        TurnStatus::Completed => TurnOutcome::Completed,
        TurnStatus::Interrupted => TurnOutcome::Interrupted,
        TurnStatus::Failed | TurnStatus::InProgress => TurnOutcome::Failed {
            message: turn
                .error
                .map_or_else(|| String::from("the turn failed"), |e| e.message),
        },
    };
    ThreadEvent::TurnCompleted {
        turn: TurnId(turn.id),
        outcome,
    }
}

fn plan_step(step: WirePlanStep) -> ChecklistEntry {
    ChecklistEntry {
        text: step.step,
        status: match step.status {
            WirePlanStatus::Pending => ChecklistStatus::Pending,
            WirePlanStatus::InProgress => ChecklistStatus::InProgress,
            WirePlanStatus::Completed => ChecklistStatus::Completed,
        },
    }
}

fn item_event(
    thread: String,
    turn_id: String,
    phase: ItemPhase,
    item: ThreadItem,
) -> Inbound {
    let turn = TurnId(turn_id);
    let event = match item {
        ThreadItem::AgentMessage { text } => {
            ThreadEvent::AgentMessage { turn, text }
        }
        ThreadItem::Reasoning { summary } => ThreadEvent::Reasoning {
            turn,
            text: summary.join("\n\n"),
        },
        ThreadItem::CollabAgentToolCall {
            id,
            tool,
            sender_thread_id,
            receiver_thread_ids,
            prompt,
            agents_states,
        } if sender_thread_id == thread => {
            ThreadEvent::Child(ChildEvent::Collaboration {
                spawn: (tool == "spawnAgent").then(|| SpawnCall {
                    call: ToolCallId(id),
                    description: prompt.filter(|text| !text.trim().is_empty()),
                    agents: receiver_thread_ids
                        .into_iter()
                        .map(ThreadId)
                        .collect(),
                }),
                states: agents_states
                    .into_iter()
                    .map(|(id, state)| (ThreadId(id), state))
                    .collect(),
            })
        }
        ThreadItem::CollabAgentToolCall { .. } => ThreadEvent::Started,
        ThreadItem::SubAgentActivity {
            id,
            agent_thread_id,
            agent_path,
            kind,
        } => ThreadEvent::Child(ChildEvent::Activity {
            call: ToolCallId(id),
            agent: ThreadId(agent_thread_id),
            path: agent_path,
            activity: kind,
        }),
        ThreadItem::CommandExecution {
            id,
            command,
            status,
        } => {
            tool_event(turn, id, phase, ToolKind::Command { command }, &status)
        }
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
            &status,
        ),
    };
    on_thread(thread, event)
}

fn tool_event(
    turn: TurnId,
    id: String,
    phase: ItemPhase,
    tool: ToolKind,
    status: &str,
) -> ThreadEvent {
    ThreadEvent::ToolCall {
        turn,
        call: ToolCallId(id),
        phase: match phase {
            ItemPhase::Started => ToolPhase::Started { tool },
            ItemPhase::Completed => ToolPhase::Finished {
                outcome: match status {
                    "completed" => ToolOutcome::Succeeded,
                    "declined" => ToolOutcome::Declined,
                    _ => ToolOutcome::Failed,
                },
            },
        },
    }
}

fn thread_unknown(raw: Value) -> CodexEvent {
    let thread = raw
        .pointer("/params/threadId")
        .or_else(|| raw.pointer("/params/thread/id"))
        .and_then(Value::as_str);
    match thread {
        Some(id) => CodexEvent::Thread {
            thread: ThreadId(id.to_owned()),
            event: ThreadEvent::Unknown(raw),
        },
        None => CodexEvent::Unknown(raw),
    }
}

#[derive(Debug, Clone)]
pub enum Call {
    Initialize,
    ModelList,
    ThreadStart(ThreadOptions),
    ThreadResume(ThreadId, ThreadOptions),
    TurnStart {
        thread: ThreadId,
        text: String,
        effort: Option<Effort>,
    },
    TurnInterrupt {
        thread: ThreadId,
        turn: TurnId,
    },
}

impl Call {
    #[must_use]
    pub fn open(options: ThreadOptions, resume: Option<ThreadId>) -> Self {
        match resume {
            Some(thread) => Self::ThreadResume(thread, options),
            None => Self::ThreadStart(options),
        }
    }
}

impl Call {
    #[must_use]
    pub const fn method(&self) -> &'static str {
        match self {
            Self::Initialize => INITIALIZE,
            Self::ModelList => "model/list",
            Self::ThreadStart(_) => "thread/start",
            Self::ThreadResume(_, _) => "thread/resume",
            Self::TurnStart { .. } => "turn/start",
            Self::TurnInterrupt { .. } => "turn/interrupt",
        }
    }

    fn params(&self) -> Value {
        match self {
            Self::Initialize => json!({
                "clientInfo": {"name": "conductor", "version": env!("CARGO_PKG_VERSION")},
                "capabilities": {"experimentalApi": true},
            }),
            Self::ModelList => json!({}),
            Self::ThreadStart(opts) => thread_params(None, opts),
            Self::ThreadResume(thread, opts) => {
                thread_params(Some(thread), opts)
            }
            Self::TurnStart {
                thread,
                text,
                effort,
            } => {
                let input = Value::Array(Vec::from([
                    json!({"type": "text", "text": text}),
                ]));
                let mut params = json!({"threadId": thread.0, "input": input});
                if let Some(level) = effort {
                    params["effort"] = Value::from(level.wire());
                }
                params
            }
            Self::TurnInterrupt { thread, turn } => {
                json!({"threadId": thread.0, "turnId": turn.0})
            }
        }
    }
}

fn thread_params(resume: Option<&ThreadId>, opts: &ThreadOptions) -> Value {
    let mut params = json!({
        "approvalPolicy": opts.approval_policy,
        "sandbox": opts.sandbox,
        "cwd": opts.cwd,
        "config": {
            "features": {"default_mode_request_user_input": true},
            "tools": {"update_plan": {"enabled": true}},
            "model_reasoning_summary": "auto",
        },
    });
    if let Some(thread) = resume {
        params["threadId"] = Value::from(thread.0.as_str());
    }
    if let Some(model) = &opts.model {
        params["model"] = Value::from(model.as_str());
    }
    if let Some(instructions) = &opts.instructions {
        params["developerInstructions"] = Value::from(instructions.as_str());
    }
    if let Some(effort) = &opts.effort {
        params["config"]["model_reasoning_effort"] = Value::from(effort.wire());
    }
    params
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

pub fn models(result: Value) -> serde_json::Result<Models> {
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
                efforts: Effort::listed(
                    m.supported_reasoning_efforts
                        .iter()
                        .map(|e| e.reasoning_effort.as_str()),
                ),
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

pub fn thread_started(
    result: Value,
) -> serde_json::Result<(ThreadId, ThreadSettings)> {
    let r = serde_json::from_value::<ThreadResponse>(result)?;
    Ok((
        ThreadId(r.thread.id),
        ThreadSettings {
            model: r.model,
            effort: r.reasoning_effort.as_deref().and_then(Effort::parse),
        },
    ))
}

pub fn turn_id(result: Value) -> serde_json::Result<TurnId> {
    let r = serde_json::from_value::<TurnResponse>(result)?;
    Ok(TurnId(r.turn.id))
}

pub fn user_agent(result: Value) -> serde_json::Result<String> {
    let r = serde_json::from_value::<InitializeResponse>(result)?;
    Ok(r.user_agent)
}

pub enum Outbound {
    Request { id: i64, call: Call },
    Initialized,
    Answer { rpc_id: RequestId, result: Value },
    Reject { rpc_id: RequestId, message: String },
}

#[derive(Debug)]
pub enum AnswerError {
    NotApplicable,
    Encode(serde_json::Error),
}

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
}

pub fn answer(
    shape: &AnswerShape,
    decision: Decision,
) -> Result<Value, AnswerError> {
    match (shape, decision) {
        (AnswerShape::UserInput(ids), Decision::Answer(answers))
            if ids.len() == answers.len() =>
        {
            encoded(UserInputAnswer {
                answers: ids
                    .iter()
                    .cloned()
                    .zip(answers)
                    .map(|(id, answer)| {
                        (id, QuestionAnswer { answers: answer.0 })
                    })
                    .collect(),
            })
        }
        (AnswerShape::UserInput(_), _)
        | (AnswerShape::Permissions(_), Decision::Answer(_)) => {
            Err(AnswerError::NotApplicable)
        }
        (AnswerShape::Command | AnswerShape::FileChange, chosen) => {
            encoded(PlainAnswer {
                decision: plain(&chosen)?,
            })
        }
        (AnswerShape::Permissions(requested), Decision::Accept) => {
            encoded(PermissionGrant {
                permissions: requested.clone(),
                scope: GrantScope::Turn,
            })
        }
        (AnswerShape::Permissions(requested), Decision::AcceptForSession) => {
            encoded(PermissionGrant {
                permissions: requested.clone(),
                scope: GrantScope::Session,
            })
        }
        (AnswerShape::Permissions(_), Decision::Decline) => {
            encoded(NoPermissions {
                permissions: Value::Object(serde_json::Map::new()),
            })
        }
    }
}

const fn plain(decision: &Decision) -> Result<PlainDecision, AnswerError> {
    match decision {
        Decision::Accept => Ok(PlainDecision::Accept),
        Decision::AcceptForSession => Ok(PlainDecision::AcceptForSession),
        Decision::Decline => Ok(PlainDecision::Decline),
        Decision::Answer(_) => Err(AnswerError::NotApplicable),
    }
}

#[derive(Serialize)]
struct UserInputAnswer {
    answers: BTreeMap<String, QuestionAnswer>,
}

#[derive(Serialize)]
struct QuestionAnswer {
    answers: Vec<String>,
}

#[derive(Serialize)]
struct NoPermissions {
    permissions: Value,
}

fn encoded<T: Serialize>(answer: T) -> Result<Value, AnswerError> {
    serde_json::to_value(answer).map_err(AnswerError::Encode)
}

const METHOD_NOT_FOUND: i64 = 32_601_i64.wrapping_neg();

#[derive(Serialize, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

pub fn encode(msg: Outbound) -> serde_json::Result<String> {
    serde_json::to_string(&match msg {
        Outbound::Request { id, call } => json!({
            "id": id, "method": call.method(), "params": call.params(),
        }),
        Outbound::Initialized => json!({"method": "initialized"}),
        Outbound::Answer { rpc_id, result } => {
            json!({"id": rpc_id, "result": result})
        }
        Outbound::Reject { rpc_id, message } => json!({
            "error": {"code": METHOD_NOT_FOUND, "message": message}, "id": rpc_id,
        }),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use agents::{Answer, Effort, Question, QuestionOption, Selection};
    use serde_json::Value;

    use super::{
        AnswerError, AnswerShape, ApprovalId, ApprovalKind, ApprovalPolicy,
        Call, CodexEvent, Decision, Inbound, Outbound, PermissionAsk,
        RequestId, Sandbox, ThreadEvent, ThreadId, ThreadOptions, TurnId,
        answer, encode, encoded, parse_line,
    };

    type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

    fn json(text: &str) -> serde_json::Result<Value> {
        serde_json::from_str(text)
    }

    fn sent(msg: Outbound) -> serde_json::Result<Value> {
        json(&encode(msg)?)
    }

    fn request(call: Call) -> serde_json::Result<Value> {
        sent(Outbound::Request { id: 7, call })
    }

    fn opts() -> ThreadOptions {
        ThreadOptions {
            instructions: Some(String::from("Be brief.")),
            cwd: "/tmp/s2-scratch".into(),
            model: Some(String::from("gpt-6-luna")),
            effort: Some(Effort::Low),
            approval_policy: ApprovalPolicy::OnRequest,
            sandbox: Sandbox::ReadOnly,
        }
    }

    #[test]
    fn request_ids_accept_only_integer_and_string_shapes() -> TestResult<()> {
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
                .map(|(wire, _)| json(wire).map(|read| RequestId::read(&read)))
                .collect::<serde_json::Result<Vec<Option<RequestId>>>>()?,
            cases
                .into_iter()
                .map(|(_, expected)| expected)
                .collect::<Vec<Option<RequestId>>>()
        );
        Ok(())
    }

    #[test]
    fn replies_keep_a_null_result_and_a_string_id()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            encode(Outbound::Answer {
                rpc_id: RequestId::String(String::from("rpc-1")),
                result: Value::Null,
            })?,
            r#"{"id":"rpc-1","result":null}"#
        );
        Ok(())
    }

    #[test]
    fn thread_start_omits_absent_model_thread_effort_and_instructions()
    -> TestResult<()> {
        let options = ThreadOptions {
            model: None,
            effort: None,
            instructions: None,
            ..opts()
        };
        assert_eq!(
            request(Call::ThreadStart(options))?,
            json(
                r#"{"id":7,"method":"thread/start","params":{
                "approvalPolicy":"on-request","sandbox":"read-only","cwd":"/tmp/s2-scratch",
                "config":{"features":{"default_mode_request_user_input":true},
                "model_reasoning_summary":"auto",
                "tools":{"update_plan":{"enabled":true}}}}}"#
            )?
        );
        Ok(())
    }

    #[test]
    fn initialize_opts_into_the_experimental_api() -> TestResult<()> {
        assert_eq!(
            request(Call::Initialize)?,
            json(&format!(
                r#"{{"id": 7, "method": "initialize", "params": {{
                "clientInfo": {{"name": "conductor", "version": "{}"}},
                "capabilities": {{"experimentalApi": true}}}}}}"#,
                env!("CARGO_PKG_VERSION")
            ))?
        );
        assert_eq!(
            sent(Outbound::Initialized)?,
            json(r#"{"method": "initialized"}"#)?
        );
        Ok(())
    }

    #[test]
    fn thread_start_and_resume_carry_policy_sandbox_and_effort()
    -> TestResult<()> {
        assert_eq!(
            request(Call::ThreadStart(opts()))?,
            json(
                r#"{"id": 7, "method": "thread/start", "params": {
                "approvalPolicy": "on-request", "sandbox": "read-only", "cwd": "/tmp/s2-scratch",
                "model": "gpt-6-luna", "developerInstructions": "Be brief.",
                "config": {"model_reasoning_effort": "low",
                "model_reasoning_summary": "auto", "features": {"default_mode_request_user_input": true},
                "tools": {"update_plan": {"enabled": true}}}}}"#
            )?
        );
        assert_eq!(
            request(Call::ThreadResume(
                ThreadId(String::from("th_1")),
                opts()
            ))?,
            json(
                r#"{"id": 7, "method": "thread/resume", "params": {
                "threadId": "th_1", "approvalPolicy": "on-request", "sandbox": "read-only",
                "cwd": "/tmp/s2-scratch", "model": "gpt-6-luna",
                "developerInstructions": "Be brief.",
                "config": {"model_reasoning_effort": "low",
                "model_reasoning_summary": "auto", "features": {"default_mode_request_user_input": true},
                "tools": {"update_plan": {"enabled": true}}}}}"#
            )?
        );
        Ok(())
    }

    #[test]
    fn turn_requests() -> TestResult<()> {
        assert_eq!(
            request(Call::TurnStart {
                thread: ThreadId(String::from("th_1")),
                text: String::from("create a.txt"),
                effort: None,
            })?,
            json(
                r#"{"id": 7, "method": "turn/start", "params": {
                "threadId": "th_1", "input": [{"type": "text", "text": "create a.txt"}]}}"#
            )?
        );
        assert_eq!(
            request(Call::TurnStart {
                thread: ThreadId(String::from("th_1")),
                text: String::from("pong"),
                effort: Some(Effort::High),
            })?,
            json(
                r#"{"id": 7, "method": "turn/start", "params": {
                "threadId": "th_1", "input": [{"type": "text", "text": "pong"}],
                "effort": "high"}}"#
            )?
        );
        assert_eq!(
            request(Call::TurnInterrupt {
                thread: ThreadId(String::from("th_1")),
                turn: TurnId(String::from("tu_2"))
            })?,
            json(
                r#"{"id": 7, "method": "turn/interrupt", "params": {"threadId": "th_1", "turnId": "tu_2"}}"#
            )?
        );
        assert_eq!(
            request(Call::ModelList)?,
            json(r#"{"id": 7, "method": "model/list", "params": {}}"#)?
        );
        Ok(())
    }

    fn answered(shape: &AnswerShape, decision: Decision) -> TestResult<Value> {
        let result = answer(shape, decision).map_err(|e| format!("{e:?}"))?;
        Ok(sent(Outbound::Answer {
            rpc_id: RequestId::Integer(0),
            result,
        })?)
    }

    fn plain_answers(shape: &AnswerShape) -> TestResult<Vec<Value>> {
        [
            Decision::Accept,
            Decision::AcceptForSession,
            Decision::Decline,
        ]
        .into_iter()
        .map(|decision| answered(shape, decision))
        .collect::<TestResult<Vec<Value>>>()
    }

    #[test]
    fn command_and_file_change_decisions() -> TestResult<()> {
        let expected = ["accept", "acceptForSession", "decline"]
            .into_iter()
            .map(|wire| {
                json(&format!(
                    r#"{{"id": 0, "result": {{"decision": "{wire}"}}}}"#
                ))
            })
            .collect::<serde_json::Result<Vec<Value>>>()?;
        assert_eq!(plain_answers(&AnswerShape::Command)?, expected);
        assert_eq!(plain_answers(&AnswerShape::FileChange)?, expected);
        Ok(())
    }

    #[test]
    fn permissions_grant_back_what_was_requested() -> TestResult<()> {
        let shape = AnswerShape::Permissions(json(
            r#"{"network": {"enabled": true}}"#,
        )?);
        assert_eq!(
            answered(&shape, Decision::Accept)?,
            json(
                r#"{"id": 0, "result": {"permissions": {"network": {"enabled": true}}, "scope": "turn"}}"#
            )?
        );
        assert_eq!(
            answered(&shape, Decision::AcceptForSession)?,
            json(
                r#"{"id": 0, "result": {"permissions": {"network": {"enabled": true}}, "scope": "session"}}"#
            )?
        );
        assert_eq!(
            answered(&shape, Decision::Decline)?,
            json(r#"{"id": 0, "result": {"permissions": {}}}"#)?
        );
        Ok(())
    }

    const USER_INPUT: &str = r#"{"method": "item/tool/requestUserInput", "id": 0, "params": {"threadId": "th_1", "turnId": "tu_1", "itemId": "call_1", "questions": [{"id": "preferred_color", "header": "Color", "question": "Which color do you prefer?", "isOther": true, "isSecret": false, "options": [{"label": "Red", "description": "Choose Red."}, {"label": "Blue", "description": "Choose Blue."}]}], "isBlocking": false, "autoResolutionMs": null}}"#;

    #[test]
    fn question_keys_are_distinct_even_when_display_text_repeats()
    -> TestResult<()> {
        let mut request = json(USER_INPUT)?;
        let questions = request
            .pointer_mut("/params/questions")
            .and_then(Value::as_array_mut)
            .ok_or("missing questions")?;
        let first = questions.first().ok_or("missing first question")?.clone();
        questions.push(first.clone());
        assert!(matches!(
            parse_line(&request.to_string())?,
            Inbound::Unsupported {
                rpc_id: RequestId::Integer(0),
                ..
            }
        ));

        let questions = request
            .pointer_mut("/params/questions")
            .and_then(Value::as_array_mut)
            .ok_or("missing questions")?;
        let _ = questions.pop();
        let mut second = first;
        *second.get_mut("id").ok_or("missing question ID")? =
            Value::String(String::from("alternate_color"));
        questions.push(second);
        let Inbound::Approval { shape, .. } = parse_line(&request.to_string())?
        else {
            return Err("expected two answerable questions".into());
        };
        let red = Answer(Vec::from([String::from("Red")]));
        let blue = Answer(Vec::from([String::from("Blue")]));
        assert!(matches!(
            answer(&shape, Decision::Answer(Vec::from([red.clone()]))),
            Err(AnswerError::NotApplicable)
        ));
        assert!(matches!(
            answer(
                &shape,
                Decision::Answer(Vec::from([
                    red.clone(),
                    blue.clone(),
                    red.clone()
                ]))
            ),
            Err(AnswerError::NotApplicable)
        ));
        assert_eq!(
            answered(&shape, Decision::Answer(Vec::from([red, blue])))?,
            json(
                r#"{"id":0,"result":{"answers":{"preferred_color":{"answers":["Red"]},"alternate_color":{"answers":["Blue"]}}}}"#
            )?
        );
        Ok(())
    }

    #[test]
    fn a_secret_question_is_declined_without_its_content()
    -> Result<(), Box<dyn std::error::Error>> {
        let line =
            USER_INPUT.replace(r#""isSecret": false"#, r#""isSecret": true"#);
        assert_eq!(
            parse_line(&line)?,
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
        Ok(())
    }

    #[test]
    fn user_input_is_a_question_answered_by_question_id()
    -> Result<(), Box<dyn std::error::Error>> {
        let (shape, event) = match parse_line(USER_INPUT)? {
            Inbound::Approval { shape, event, .. } => Some((shape, event)),
            Inbound::Event(_)
            | Inbound::Response { .. }
            | Inbound::Resolved { .. }
            | Inbound::Unsupported { .. }
            | Inbound::Declined { .. } => None,
        }
        .ok_or("expected a value")?;
        assert_eq!(
            event,
            ThreadEvent::ApprovalRequested {
                id: ApprovalId(String::from("integer:0")),
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
            }
        );
        assert_eq!(
            answered(
                &shape,
                Decision::Answer(Vec::from([Answer(Vec::from([
                    String::from("Blue")
                ]))]))
            )?,
            json(
                r#"{"id": 0, "result": {"answers": {"preferred_color": {"answers": ["Blue"]}}}}"#
            )?
        );
        assert_eq!(
            match answer(&shape, Decision::Accept) {
                Err(AnswerError::NotApplicable) => "not applicable",
                Err(AnswerError::Encode(_)) => "encode",
                Ok(_) => "answered",
            },
            "not applicable"
        );
        Ok(())
    }

    #[test]
    fn an_answer_that_does_not_encode_is_an_encode_error() {
        assert_eq!(
            match encoded(BTreeMap::from([(
                (1, 2),
                "a tuple is not a JSON key"
            )])) {
                Err(AnswerError::Encode(e)) => e.to_string(),
                Err(AnswerError::NotApplicable) =>
                    String::from("not applicable"),
                Ok(value) => value.to_string(),
            },
            "key must be a string"
        );
    }

    #[test]
    fn unsupported_request_is_rejected_with_method_not_found() -> TestResult<()>
    {
        assert_eq!(
            sent(Outbound::Reject {
                rpc_id: RequestId::Integer(4),
                message: String::from(
                    "conductor does not handle item/tool/call"
                ),
            })?,
            json(
                r#"{"id": 4, "error": {"code": -32601,
                "message": "conductor does not handle item/tool/call"}}"#
            )?
        );
        Ok(())
    }

    #[test]
    fn command_approval_accepts_extra_wire_fields()
    -> Result<(), Box<dyn std::error::Error>> {
        let line = r#"{"method":"item/commandExecution/requestApproval","id":0,"params":{"kind":"command","threadId":"th_1","turnId":"tu_1","itemId":"exec-1","startedAtMs":1,"reason":"May I?","command":"printf hi > a.txt","cwd":"/tmp/s","availableDecisions":["accept",{"acceptWithExecpolicyAmendment":{"execpolicy_amendment":["printf"]}},"cancel"]}}"#;
        assert_eq!(
            parse_line(line)?,
            Inbound::Approval {
                rpc_id: RequestId::Integer(0),
                thread: ThreadId(String::from("th_1")),
                shape: AnswerShape::Command,
                event: ThreadEvent::ApprovalRequested {
                    id: ApprovalId(String::from("integer:0")),
                    turn: TurnId(String::from("tu_1")),
                    item: String::from("exec-1"),
                    kind: ApprovalKind::Command {
                        command: Some(String::from("printf hi > a.txt")),
                        cwd: Some(String::from("/tmp/s")),
                        reason: Some(String::from("May I?")),
                    },
                },
            }
        );
        Ok(())
    }

    #[test]
    fn a_permissions_request_lists_what_it_asks_for() -> TestResult<()> {
        let line = r#"{"method":"item/permissions/requestApproval","id":3,"params":{"threadId":"th_1","turnId":"tu_1","itemId":"exec-2","startedAtMs":1,"cwd":"/w","reason":"to fetch","permissions":{"unsupported":true,"network":{"enabled":true,"future":true},"fileSystem":{"write":["/home/u/.cargo"],"entries":[{"access":"write","path":{"type":"special","value":{"kind":"slashTmp"}}},{"access":"read","path":{"type":"path","path":"/etc/hosts"}}]}}}}"#;
        let inbound = parse_line(line)?;
        if let Inbound::Approval { shape, .. } = &inbound {
            let grant = answer(shape, Decision::Accept).ok();
            assert_eq!(
                grant,
                Some(json(
                    r#"{"permissions":{"network":{"enabled":true},"fileSystem":{"write":["/home/u/.cargo"],"entries":[{"access":"write","path":{"type":"special","value":{"kind":"slashTmp"}}},{"access":"read","path":{"type":"path","path":"/etc/hosts"}}]}},"scope":"turn"}"#
                )?)
            );
        }
        assert_eq!(
            match inbound {
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
                    PermissionAsk::Read(Vec::from([String::from(
                        "/etc/hosts"
                    )])),
                    PermissionAsk::Write(Vec::from([
                        String::from("/home/u/.cargo"),
                        String::from("/tmp"),
                    ])),
                ])
            ))
        );
        Ok(())
    }

    #[test]
    fn unknown_notification_keeps_its_thread()
    -> Result<(), Box<dyn std::error::Error>> {
        let line = r#"{"method":"thread/brandNew","params":{"threadId":"th_1","x":1}}"#;
        assert_eq!(
            parse_line(line)?,
            Inbound::Event(CodexEvent::Thread {
                thread: ThreadId(String::from("th_1")),
                event: ThreadEvent::Unknown(json(line)?),
            })
        );
        Ok(())
    }

    #[test]
    fn known_method_with_drifted_params_becomes_unknown()
    -> Result<(), Box<dyn std::error::Error>> {
        let line = r#"{"method":"turn/completed","params":{"threadId":"th_1","turn":{"id":"tu_1","items":[],"status":"paused"}}}"#;
        assert_eq!(
            parse_line(line)?,
            Inbound::Event(CodexEvent::Thread {
                thread: ThreadId(String::from("th_1")),
                event: ThreadEvent::Unknown(json(line)?),
            })
        );
        Ok(())
    }

    #[test]
    fn unknown_server_request_still_yields_its_id()
    -> Result<(), Box<dyn std::error::Error>> {
        let line = r#"{"method":"item/frobnicate","id":9,"params":{"threadId":"th_1"}}"#;
        assert_eq!(
            parse_line(line)?,
            Inbound::Unsupported {
                rpc_id: RequestId::Integer(9),
                method: String::from("item/frobnicate"),
                event: CodexEvent::Thread {
                    thread: ThreadId(String::from("th_1")),
                    event: ThreadEvent::Unknown(json(line)?),
                },
            }
        );
        Ok(())
    }

    #[test]
    fn resolved_names_the_request_id() -> Result<(), Box<dyn std::error::Error>>
    {
        let line = r#"{"method":"serverRequest/resolved","params":{"threadId":"th_1","requestId":0}}"#;
        assert_eq!(
            parse_line(line)?,
            Inbound::Resolved {
                thread: ThreadId(String::from("th_1")),
                rpc_id: RequestId::Integer(0),
            }
        );
        Ok(())
    }

    #[test]
    fn tool_notifications_emit_shared_phases_and_file_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        use agents::{
            Change, FileChange, ToolCallId, ToolKind, ToolOutcome, ToolPhase,
        };
        let cases = [
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
        ];
        let (got, want) = cases
            .into_iter()
            .map(|(line, phase)| {
                Ok((
                    parse_line(line)?,
                    Inbound::Event(CodexEvent::Thread {
                        thread: super::ThreadId(String::from("th")),
                        event: super::ThreadEvent::ToolCall {
                            turn: super::TurnId(String::from("tu")),
                            call: ToolCallId(String::from("call")),
                            phase,
                        },
                    }),
                ))
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?
            .into_iter()
            .unzip::<Inbound, Inbound, Vec<Inbound>, Vec<Inbound>>();
        assert_eq!(got, want);
        Ok(())
    }

    #[test]
    fn completed_turns_emit_shared_outcomes_with_failure_messages()
    -> Result<(), Box<dyn std::error::Error>> {
        use agents::TurnOutcome;
        let cases = [
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
        ];
        let (got, want) = cases
            .into_iter()
            .map(|(line, outcome)| {
                Ok((
                    parse_line(line)?,
                    Inbound::Event(CodexEvent::Thread {
                        thread: super::ThreadId(String::from("th")),
                        event: super::ThreadEvent::TurnCompleted {
                            turn: super::TurnId(String::from("tu")),
                            outcome,
                        },
                    }),
                ))
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?
            .into_iter()
            .unzip::<Inbound, Inbound, Vec<Inbound>, Vec<Inbound>>();
        assert_eq!(got, want);
        Ok(())
    }
    #[test]
    fn hidden_models_are_left_out_and_the_default_is_marked() -> TestResult<()>
    {
        let listed = serde_json::from_str::<Value>(
            r#"{"data": [
                {"id": "gpt-6", "displayName": "GPT-6", "description": "a", "isDefault": false, "supportedReasoningEfforts": []},
                {"id": "gpt-6-luna", "displayName": "Luna", "description": "b", "isDefault": true, "supportedReasoningEfforts": [{"reasoningEffort": "high"}]},
                {"id": "gpt-5", "displayName": "GPT-5", "description": "c", "hidden": true, "supportedReasoningEfforts": []}
            ]}"#,
        )?;
        let models = super::models(listed)?;
        assert_eq!(
            models
                .choices
                .iter()
                .map(|choice| (choice.id.0.as_str(), choice.efforts.clone()))
                .collect::<Vec<_>>(),
            Vec::from([
                ("gpt-6", Vec::new()),
                ("gpt-6-luna", Vec::from([Effort::High]))
            ])
        );
        assert_eq!(
            models.default,
            Some(agents::ModelId(String::from("gpt-6-luna")))
        );
        Ok(())
    }

    #[test]
    fn the_user_agent_is_read_from_initialize() -> TestResult<()> {
        let result = serde_json::from_str::<Value>(
            r#"{"userAgent": "conductor/0.166.0 (Ubuntu 24.4.0; x86_64)"}"#,
        )?;
        assert_eq!(
            super::user_agent(result)?,
            "conductor/0.166.0 (Ubuntu 24.4.0; x86_64)"
        );
        Ok(())
    }
}
