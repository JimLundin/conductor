//! The stdio boundary. Each stdout line is parsed into the few frames we read
//! and mapped into our own types here, and every line we write is built here.
//! Frames and fields we do not model are ignored or passed on as `Unknown`.

use alloc::collections::BTreeSet;

use agents::Exit;
use proto::{
    Answer, Change, ChecklistEntry, ChecklistStatus, Effort, FileChange,
    ModelChoice, ModelId, Models, Question, QuestionOption, Selection,
    SubagentOutcome, ToolKind, ToolOutcome, TurnOutcome,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::prelude::*;

/// A Claude session id, as printed in `system/init` and accepted by `--resume`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(pub String);

/// A top-level transcript-entry UUID, accepted by `--resume-session-at`.
/// This is not the API's `message.id` (`msg_…`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageId(pub String);

/// The CLI's `request_id` for a `can_use_tool` request.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PermissionRequestId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    Default,
    AcceptEdits,
}

impl PermissionMode {
    /// The name `--permission-mode` and `set_permission_mode` take.
    #[must_use]
    pub const fn wire(&self) -> &str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Run the tool with the input the CLI proposed.
    Allow,
    /// Allow, and add the request's session grant (`for_session`) to the
    /// session's permissions. A request with no grant is allowed once.
    AllowForSession,
    /// Answer an `AskUserQuestion` request, one answer per question.
    Answer(Vec<Answer>),
    Deny {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Assistant,
    User,
}

/// The part of each tool's `input` worth showing, as the shared tool kind.
fn tool_kind(name: &str, input: &Value) -> ToolKind {
    let shown =
        match name {
            "Bash" => field(input, "command")
                .map(|command| ToolKind::Command { command }),
            "Write" => field(input, "file_path")
                .map(|path| one_file(path, Change::Write)),
            "Edit" | "MultiEdit" => field(input, "file_path")
                .map(|path| one_file(path, Change::Edit)),
            "Read" => {
                field(input, "file_path").map(|detail| other_tool(name, detail))
            }
            "Grep" | "Glob" => {
                field(input, "pattern").map(|detail| other_tool(name, detail))
            }
            "WebFetch" => {
                field(input, "url").map(|detail| other_tool(name, detail))
            }
            "Agent" | "Task" => field(input, "description")
                .map(|detail| other_tool(name, detail)),
            _ => None,
        };
    shown.unwrap_or_else(|| other_tool(name, input.to_string()))
}

fn one_file(path: String, change: Change) -> ToolKind {
    ToolKind::FileChange {
        files: Vec::from([FileChange { path, change }]),
    }
}

fn other_tool(name: &str, detail: String) -> ToolKind {
    ToolKind::Other {
        name: String::from(name),
        detail,
    }
}

fn field(input: &Value, name: &str) -> Option<String> {
    input.get(name).and_then(Value::as_str).map(str::to_owned)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Text(String),
    Thinking(String),
    ToolUse {
        id: String,
        name: String,
        tool: ToolKind,
    },
    ToolResult {
        tool_use_id: String,
        content: Value,
        outcome: ToolOutcome,
    },
    /// A `TodoWrite` call: the whole checklist as the model wrote it.
    Checklist(Vec<ChecklistEntry>),
    Other(Value),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeEvent {
    /// The CLI read a user message and began its turn (`system/init`, which
    /// it prints once per turn with the same `session_id`).
    TurnStarted {
        session_id: SessionId,
    },
    Message {
        role: Role,
        content: Vec<Block>,
        uuid: Option<MessageId>,
    },
    /// A piece of assistant text as it streams (`--include-partial-messages`).
    /// The whole text follows in a `Message`.
    StreamDelta {
        text: String,
    },
    /// A piece of a thinking block as it streams. The CLI sends its text
    /// only when it asks the API for summarized thinking (PROTOCOL.md); the
    /// whole block follows as `Block::Thinking` in a `Message`.
    ThinkingDelta {
        text: String,
    },
    PermissionRequested {
        agent: Option<String>,
        id: PermissionRequestId,
        tool_name: String,
        tool: ToolKind,
        /// The `ToolUse` block this asks about.
        tool_use_id: String,
        /// What `AllowForSession` adds, in the CLI's rule syntax
        /// (`Bash(git status)`), when the CLI suggests a rule or a mode.
        for_session: Option<String>,
    },
    /// A `can_use_tool` request for `AskUserQuestion`. Answer it with
    /// [`PermissionDecision::Answer`]; the CLI's `PermissionCancelled`
    /// withdraws it like any request.
    QuestionAsked {
        agent: Option<String>,
        id: PermissionRequestId,
        questions: Vec<Question>,
        tool_use_id: String,
    },
    /// The CLI withdrew a permission request; answering it is now an error.
    PermissionCancelled {
        id: PermissionRequestId,
    },
    /// An interrupted turn reports `error_during_execution`; only
    /// `terminal_reason` (`aborted_streaming`, `aborted_tools`) says why.
    TurnFinished {
        subtype: String,
        outcome: TurnOutcome,
        terminal_reason: Option<String>,
        session_id: SessionId,
        /// Why the CLI could not run the turn, as `No conversation found
        /// with session ID: …` for a `--resume` with no transcript.
        errors: Vec<String>,
        /// Last top-level user or assistant entry of this turn, including
        /// an interrupted marker. None means no safe retained prefix is known.
        last_message: Option<MessageId>,
    },
    /// The `Agent` call `tool_use_id` started the sub-agent `agent`
    /// (`system/task_started` of a `local_agent`).
    SubagentStarted {
        agent: String,
        tool_use_id: String,
        description: String,
        kind: Option<String>,
    },
    /// A sub-agent's own message, which its `agent_id` names.
    SubagentMessage {
        agent: String,
        role: Role,
        content: Vec<Block>,
    },
    /// A child entry without `agent_id` can be attributed by its parent call.
    UnidentifiedSubagentMessage {
        parent: String,
        role: Role,
        content: Vec<Block>,
    },
    /// A task ended (`system/task_notification`). The frame doesn't say
    /// whether the task is a sub-agent; the session's state does.
    SubagentEnded {
        agent: String,
        outcome: SubagentOutcome,
        summary: Option<String>,
    },
    Exited {
        status: Exit,
    },
    Unknown(Value),
}

/// What one stdout line means to the session actor.
#[derive(Debug, Clone, PartialEq)]
pub enum Inbound {
    Event(ClaudeEvent),
    /// A `can_use_tool` request. The actor keeps `open` for the answer and
    /// emits `event`, a `PermissionRequested` or a `QuestionAsked`.
    Permission {
        id: PermissionRequestId,
        open: Open,
        parent_tool_use_id: Option<String>,
        event: ClaudeEvent,
    },
    /// Answer to a control request we sent.
    ControlResponse {
        request_id: String,
        outcome: Result<Value, String>,
    },
    /// A control request we do not serve (hooks, SDK MCP servers, or a subtype
    /// we do not know). It must still get an error reply or the
    /// CLI waits forever.
    UnsupportedControlRequest {
        request_id: String,
        subtype: String,
    },
}

/// The stdout frames we read, by `type`. A frame of any other type, or one
/// whose fields don't fit, is passed on as `Unknown`.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Frame {
    System(System),
    Assistant(Entry),
    User(Entry),
    StreamEvent {
        event: StreamEvent,
        parent_tool_use_id: Option<String>,
    },
    Result {
        subtype: String,
        is_error: Option<bool>,
        terminal_reason: Option<String>,
        session_id: String,
        errors: Option<Vec<String>>,
    },
    ControlRequest {
        request_id: String,
        request: Value,
        parent_tool_use_id: Option<String>,
    },
    ControlResponse {
        response: ControlResponse,
    },
    ControlCancelRequest {
        request_id: String,
    },
}

/// The `system` frames we read, by `subtype`: `init`, which the CLI prints
/// once per turn, and a task's start and end.
#[derive(Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
enum System {
    Init {
        session_id: String,
    },
    /// A task the CLI runs: a sub-agent is a `local_agent` started by the
    /// `Agent` call `tool_use_id`.
    TaskStarted {
        task_id: String,
        tool_use_id: Option<String>,
        description: String,
        task_type: Option<String>,
        subagent_type: Option<String>,
    },
    /// Any task's end, which doesn't say the task's type.
    TaskNotification {
        task_id: String,
        status: TaskEnd,
        summary: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum TaskEnd {
    Completed,
    Failed,
    Stopped,
}

/// The `task_type` of a sub-agent.
const LOCAL_AGENT: &str = "local_agent";

/// A transcript entry. One with a `parent_tool_use_id` is a sub-agent's,
/// and its `agent_id` names the sub-agent.
#[derive(Deserialize)]
struct Entry {
    message: Message,
    uuid: Option<String>,
    parent_tool_use_id: Option<String>,
    agent_id: Option<String>,
}

#[derive(Deserialize)]
struct Message {
    content: Vec<Value>,
}

#[derive(Deserialize)]
struct StreamEvent {
    delta: Option<Delta>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Delta {
    TextDelta { text: String },
    ThinkingDelta { thinking: String },
}

/// The one control request we serve, by `subtype`.
#[derive(Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
enum ControlRequest {
    CanUseTool {
        tool_name: String,
        input: Value,
        tool_use_id: Option<String>,
        permission_suggestions: Option<Value>,
    },
}

/// What an answer to an open `can_use_tool` request needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Open {
    /// The CLI's proposed input, which `allow` echoes as `updatedInput`.
    input: Value,
    grant: Option<Grant>,
}

/// The CLI's suggestion to allow calls like this one, made session-wide.
#[derive(Debug, Clone, PartialEq)]
struct Grant {
    label: String,
    update: Update,
}

/// An entry of `permission_suggestions` that allows calls by rule. Rules
/// are suggested for `localSettings`, which would outlive the session, so a
/// grant keeps only their rules and sends them for `session`.
/// Any other entry fails this parse and is skipped: a `setMode`, which
/// would allow every edit and change the mode behind the host's back, a
/// rule that denies or asks, and `addDirectories`.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Suggestion {
    AddRules {
        #[serde(default)]
        rules: Value,
        behavior: Behavior,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum Behavior {
    Allow,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Rule {
    tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    rule_content: Option<String>,
}

/// One entry of `updatedPermissions`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Update {
    AddRules {
        rules: Vec<Rule>,
        behavior: String,
        destination: String,
    },
}

/// `AskUserQuestion`'s input, as far as we read it.
#[derive(Deserialize)]
struct AskInput {
    questions: Vec<Asked>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Asked {
    question: String,
    header: String,
    options: Vec<QuestionOption>,
    multi_select: Option<bool>,
}

/// The tool Claude asks the user questions with.
pub const ASK_USER_QUESTION: &str = "AskUserQuestion";

/// The tool Claude keeps its checklist with. 2.1.316 offers it to a 5.x
/// model only with `CLAUDE_CODE_ENABLE_TODO_TOOLS` on and
/// `CLAUDE_CODE_ENABLE_TASKS` off (PROTOCOL.md).
const TODO_WRITE: &str = "TodoWrite";

/// `TodoWrite`'s input, as far as we read it.
#[derive(Deserialize)]
struct TodoInput {
    todos: Vec<Todo>,
}

#[derive(Deserialize)]
struct Todo {
    content: String,
    status: TodoStatus,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

fn todos(input: &Value) -> serde_json::Result<Vec<ChecklistEntry>> {
    serde_json::from_value::<TodoInput>(input.clone()).map(|parsed| {
        parsed
            .todos
            .into_iter()
            .map(|todo| ChecklistEntry {
                text: todo.content,
                status: match todo.status {
                    TodoStatus::Pending => ChecklistStatus::Pending,
                    TodoStatus::InProgress => ChecklistStatus::InProgress,
                    TodoStatus::Completed => ChecklistStatus::Completed,
                },
            })
            .collect()
    })
}

impl Rule {
    /// The rule as the CLI writes it in settings: `Bash(git status)`.
    fn label(&self) -> String {
        self.rule_content.as_ref().map_or_else(
            || self.tool_name.clone(),
            |content| format!("{}({content})", self.tool_name),
        )
    }
}

/// The session grant the CLI's suggestions offer: their allow rules, if
/// any.
fn grant(raw: Option<Value>) -> Option<Grant> {
    let suggestions = serde_json::from_value::<Vec<Value>>(raw?).ok()?;
    let mut rules = Vec::new();
    for suggestion in suggestions {
        if let Ok(Suggestion::AddRules {
            rules: proposed,
            behavior: Behavior::Allow,
        }) = serde_json::from_value(suggestion)
        {
            rules.extend(serde_json::from_value::<Vec<Rule>>(proposed).ok()?);
        }
    }
    if rules.is_empty()
        || rules.iter().any(|rule| rule.tool_name.trim().is_empty())
    {
        return None;
    }
    Some(Grant {
        label: rules
            .iter()
            .map(Rule::label)
            .collect::<Vec<String>>()
            .join(", "),
        update: Update::AddRules {
            rules,
            behavior: String::from("allow"),
            destination: String::from("session"),
        },
    })
}

fn questions(input: &Value) -> serde_json::Result<Vec<Asked>> {
    let asked = serde_json::from_value::<AskInput>(input.clone())?.questions;
    let keys = asked
        .iter()
        .map(|one| &one.question)
        .collect::<BTreeSet<&String>>();
    if keys.len() != asked.len() {
        return Err(serde::de::Error::custom("duplicate question text"));
    }
    Ok(asked)
}

#[derive(Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
enum ControlResponse {
    Success {
        request_id: String,
        response: Option<Value>,
    },
    Error {
        request_id: String,
        error: String,
    },
}

/// The content blocks we map; any other block is passed on as `Block::Other`.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: Option<Value>,
        is_error: Option<bool>,
    },
}

/// # Errors
/// Fails when the line is not JSON.
pub fn parse_line(line: &str) -> serde_json::Result<Inbound> {
    let raw = serde_json::from_str::<Value>(line)?;
    Ok(match serde_json::from_value::<Frame>(raw.clone()) {
        Ok(frame) => inbound(frame, raw),
        Err(_) => Inbound::Event(ClaudeEvent::Unknown(raw)),
    })
}

fn inbound(frame: Frame, raw: Value) -> Inbound {
    match frame {
        Frame::System(system) => Inbound::Event(system_event(system, raw)),
        Frame::Assistant(entry) => entry_event(Role::Assistant, entry),
        Frame::User(entry) => entry_event(Role::User, entry),
        // A sub-agent's stream, whose finished message follows.
        Frame::StreamEvent {
            event: StreamEvent { delta: None },
            ..
        }
        | Frame::StreamEvent {
            parent_tool_use_id: Some(_),
            ..
        } => Inbound::Event(ClaudeEvent::Unknown(raw)),
        Frame::StreamEvent {
            event:
                StreamEvent {
                    delta: Some(Delta::TextDelta { text }),
                },
            parent_tool_use_id: None,
        } => Inbound::Event(ClaudeEvent::StreamDelta { text }),
        Frame::StreamEvent {
            event:
                StreamEvent {
                    delta: Some(Delta::ThinkingDelta { thinking }),
                },
            parent_tool_use_id: None,
        } => Inbound::Event(ClaudeEvent::ThinkingDelta { text: thinking }),
        Frame::Result {
            subtype,
            is_error,
            terminal_reason,
            session_id,
            errors,
        } => Inbound::Event(ClaudeEvent::TurnFinished {
            outcome: match is_error
                .unwrap_or(false)
                .then_some(terminal_reason.as_deref())
            {
                None => TurnOutcome::Completed,
                Some(Some("aborted_streaming" | "aborted_tools")) => {
                    TurnOutcome::Interrupted
                }
                Some(_) => TurnOutcome::Failed {
                    message: subtype.clone(),
                },
            },
            subtype,
            terminal_reason,
            session_id: SessionId(session_id),
            errors: errors.unwrap_or_default(),
            last_message: None,
        }),
        Frame::ControlRequest {
            request_id,
            request,
            parent_tool_use_id,
        } => control_request(request_id, &request, parent_tool_use_id),
        Frame::ControlResponse {
            response:
                ControlResponse::Success {
                    request_id,
                    response,
                },
        } => Inbound::ControlResponse {
            request_id,
            outcome: Ok(response.unwrap_or(Value::Null)),
        },
        Frame::ControlResponse {
            response: ControlResponse::Error { request_id, error },
        } => Inbound::ControlResponse {
            request_id,
            outcome: Err(error),
        },
        Frame::ControlCancelRequest { request_id } => {
            Inbound::Event(ClaudeEvent::PermissionCancelled {
                id: PermissionRequestId(request_id),
            })
        }
    }
}

fn tool_outcome(is_error: Option<bool>) -> ToolOutcome {
    if is_error.unwrap_or(false) {
        ToolOutcome::Failed
    } else {
        ToolOutcome::Succeeded
    }
}

/// A request we can't parse, unknown or malformed, still yields its
/// `request_id`, so it gets an error reply.
fn control_request(
    request_id: String,
    request: &Value,
    parent_tool_use_id: Option<String>,
) -> Inbound {
    match serde_json::from_value::<ControlRequest>(request.clone()) {
        Ok(ControlRequest::CanUseTool {
            tool_name,
            input,
            tool_use_id: Some(tool_use_id),
            permission_suggestions,
        }) => {
            let id = PermissionRequestId(request_id);
            let (event, grant) = if tool_name == ASK_USER_QUESTION {
                let Ok(asked) = questions(&input) else {
                    return Inbound::UnsupportedControlRequest {
                        request_id: id.0,
                        subtype: String::from("invalid AskUserQuestion"),
                    };
                };
                (
                    ClaudeEvent::QuestionAsked {
                        agent: None,
                        id: id.clone(),
                        questions: asked
                            .into_iter()
                            .map(|one| Question {
                                header: one.header,
                                text: one.question,
                                options: one.options,
                                selection: if one.multi_select.unwrap_or(false)
                                {
                                    Selection::Many
                                } else {
                                    Selection::One
                                },
                            })
                            .collect(),
                        tool_use_id,
                    },
                    None,
                )
            } else {
                let offered = grant(permission_suggestions);
                (
                    ClaudeEvent::PermissionRequested {
                        agent: None,
                        id: id.clone(),
                        tool: tool_kind(&tool_name, &input),
                        tool_name,
                        tool_use_id,
                        for_session: offered
                            .as_ref()
                            .map(|session_grant| session_grant.label.clone()),
                    },
                    offered,
                )
            };
            Inbound::Permission {
                id,
                open: Open { input, grant },
                parent_tool_use_id: parent_tool_use_id.or_else(|| {
                    request
                        .get("parent_tool_use_id")
                        .and_then(Value::as_str)
                        .map(String::from)
                }),
                event,
            }
        }
        // Conductor shows a request on its tool call, so a request that names
        // none is refused like an unknown subtype.
        Ok(ControlRequest::CanUseTool {
            tool_use_id: None, ..
        }) => Inbound::UnsupportedControlRequest {
            request_id,
            subtype: String::from("can_use_tool without tool_use_id"),
        },
        Err(_) => Inbound::UnsupportedControlRequest {
            request_id,
            subtype: request
                .get("subtype")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        },
    }
}

/// A task other than a sub-agent, and the start of a sub-agent with no
/// `Agent` call, are passed on as `Unknown`.
fn system_event(system: System, raw: Value) -> ClaudeEvent {
    match system {
        System::Init { session_id } => ClaudeEvent::TurnStarted {
            session_id: SessionId(session_id),
        },
        System::TaskStarted {
            task_id,
            tool_use_id: Some(tool_use_id),
            description,
            task_type: Some(task_type),
            subagent_type,
        } if task_type == LOCAL_AGENT => ClaudeEvent::SubagentStarted {
            agent: task_id,
            tool_use_id,
            description,
            kind: subagent_type,
        },
        System::TaskStarted { .. } => ClaudeEvent::Unknown(raw),
        System::TaskNotification {
            task_id,
            status,
            summary,
        } => ClaudeEvent::SubagentEnded {
            agent: task_id,
            outcome: match status {
                TaskEnd::Completed => SubagentOutcome::Finished,
                TaskEnd::Failed => SubagentOutcome::Failed,
                TaskEnd::Stopped => SubagentOutcome::Stopped,
            },
            summary: summary.filter(|report| !report.is_empty()),
        },
    }
}

/// A sub-agent's entry that names no sub-agent is `Unknown`.
fn entry_event(role: Role, entry: Entry) -> Inbound {
    match (entry.parent_tool_use_id, entry.agent_id) {
        (None, _) => message_event(role, entry.message, entry.uuid),
        (Some(_), Some(agent)) => {
            Inbound::Event(ClaudeEvent::SubagentMessage {
                agent,
                role,
                content: entry.message.content.into_iter().map(block).collect(),
            })
        }
        (Some(parent), None) => {
            Inbound::Event(ClaudeEvent::UnidentifiedSubagentMessage {
                parent,
                role,
                content: entry.message.content.into_iter().map(block).collect(),
            })
        }
    }
}

fn message_event(
    role: Role,
    message: Message,
    uuid: Option<String>,
) -> Inbound {
    Inbound::Event(ClaudeEvent::Message {
        role,
        uuid: uuid.map(MessageId),
        content: message.content.into_iter().map(block).collect(),
    })
}

fn block(raw: Value) -> Block {
    match serde_json::from_value::<ContentBlock>(raw.clone()) {
        Ok(ContentBlock::Text { text }) => Block::Text(text),
        Ok(ContentBlock::Thinking { thinking }) => Block::Thinking(thinking),
        Ok(ContentBlock::ToolUse { id, name, input }) => {
            match (name.as_str(), todos(&input)) {
                (TODO_WRITE, Ok(steps)) => Block::Checklist(steps),
                (_, _) => Block::ToolUse {
                    tool: tool_kind(&name, &input),
                    id,
                    name,
                },
            }
        }
        Ok(ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        }) => Block::ToolResult {
            tool_use_id,
            content: content.unwrap_or(Value::Null),
            outcome: tool_outcome(is_error),
        },
        Err(_) => Block::Other(raw),
    }
}

/// The CLI versions we have checked: from the version the fixtures were
/// recorded at to the newest one these types were checked against. Widen it
/// only after checking these types against the new CLI.
const SUPPORTED_VERSIONS: [&str; 7] = [
    "2.1.316", "2.1.317", "2.1.318", "2.1.319", "2.1.320", "2.1.321", "2.1.322",
];

/// Why the CLI's version is not one we support.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnsupportedVersion {
    /// The `initialize` response names no `claude_code_version`.
    Unreported,
    Outside {
        found: String,
        supported: String,
    },
}

#[derive(Deserialize)]
struct InitializeResponse {
    claude_code_version: String,
}

#[derive(Deserialize)]
struct InitializeModels {
    models: Vec<ModelRow>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelRow {
    value: String,
    resolved_model: Option<String>,
    display_name: String,
    description: String,
    supported_effort_levels: Option<Vec<String>>,
}

/// The model and effort the CLI runs with, as `get_settings` reports them
/// in `applied`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Applied {
    pub model: String,
    pub effort: Option<String>,
}

#[derive(Deserialize)]
struct Settings {
    applied: Applied,
}

/// # Errors
/// Fails when `settings` has no `applied` model.
pub fn applied(settings: Value) -> serde_json::Result<Applied> {
    serde_json::from_value::<Settings>(settings).map(|parsed| parsed.applied)
}

/// The row of `models` that stands for whichever model is the default.
const DEFAULT_ROW: &str = "default";

/// The models an `initialize` response lists. The `default` row is not a
/// choice of its own: it names the row with the same `resolvedModel` as
/// the default.
#[must_use]
pub fn models(initialize: &Value) -> Models {
    let rows = serde_json::from_value::<InitializeModels>(initialize.clone())
        .map_or(Vec::new(), |listed| listed.models);
    let resolved = rows
        .iter()
        .find(|row| row.value == DEFAULT_ROW)
        .and_then(|row| row.resolved_model.clone());
    let named = rows
        .into_iter()
        .filter(|row| row.value != DEFAULT_ROW)
        .collect::<Vec<ModelRow>>();
    let default = named
        .iter()
        .find(|row| resolved.is_some() && row.resolved_model == resolved)
        .map(|row| ModelId(row.value.clone()));
    Models {
        choices: named
            .into_iter()
            .map(|row| ModelChoice {
                id: ModelId(row.value),
                name: row.display_name,
                description: row.description,
                efforts: row
                    .supported_effort_levels
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|level| Effort::parse(level))
                    .collect(),
            })
            .collect(),
        default,
    }
}

/// Checks the `claude_code_version` an `initialize` response reports.
///
/// # Errors
/// Fails when it reports none, or one we have not checked.
pub fn check_version(initialize: Value) -> Result<(), UnsupportedVersion> {
    let found = serde_json::from_value::<InitializeResponse>(initialize)
        .map_err(|_| UnsupportedVersion::Unreported)?
        .claude_code_version;
    if SUPPORTED_VERSIONS.contains(&found.as_str()) {
        return Ok(());
    }
    let [first, .., last] = SUPPORTED_VERSIONS;
    Err(UnsupportedVersion::Outside {
        found,
        supported: format!("{first} to {last}"),
    })
}

/// A control request we originate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCall {
    Initialize,
    Interrupt,
    SetPermissionMode(PermissionMode),
    GetSettings,
    /// `apply_flag_settings` with `effortLevel`.
    SetEffort(Effort),
}

impl ControlCall {
    #[must_use]
    pub const fn subtype(&self) -> &str {
        match self {
            Self::Initialize => "initialize",
            Self::Interrupt => "interrupt",
            Self::SetPermissionMode(_) => "set_permission_mode",
            Self::GetSettings => "get_settings",
            Self::SetEffort(_) => "apply_flag_settings",
        }
    }
}

pub enum Outbound {
    User(String),
    Control {
        request_id: String,
        call: ControlCall,
    },
    Permission {
        id: PermissionRequestId,
        decision: PermissionDecision,
        open: Open,
    },
    /// Reply to a control request we do not serve.
    ControlError {
        request_id: String,
        message: String,
    },
}

/// A line we write to stdin. Field order is the order on the wire.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Line {
    User {
        message: UserMessage,
    },
    ControlRequest {
        request_id: String,
        request: Request,
    },
    ControlResponse {
        response: Reply,
    },
}

#[derive(Serialize)]
struct UserMessage {
    role: String,
    content: Vec<TextBlock>,
}

#[derive(Serialize)]
struct TextBlock {
    text: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Serialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
enum Request {
    Initialize,
    Interrupt,
    SetPermissionMode { mode: String },
    GetSettings,
    ApplyFlagSettings { settings: FlagSettings },
}

#[derive(Serialize)]
struct FlagSettings {
    #[serde(rename = "effortLevel")]
    effort_level: String,
}

#[derive(Serialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
enum Reply {
    Success { request_id: String, response: Value },
    Error { request_id: String, error: String },
}

/// The answer to `can_use_tool`. It goes out as a `Value`, so its keys are sorted.
#[derive(Serialize)]
#[serde(tag = "behavior", rename_all = "snake_case")]
enum Verdict {
    Allow {
        #[serde(rename = "updatedInput")]
        updated_input: Value,
        #[serde(
            rename = "updatedPermissions",
            skip_serializing_if = "Option::is_none"
        )]
        updated_permissions: Option<Vec<Update>>,
    },
    Deny {
        message: String,
    },
}

/// # Errors
/// Fails when the answers to a question don't fit its input.
pub fn encode(msg: Outbound) -> serde_json::Result<String> {
    serde_json::to_string(&match msg {
        Outbound::User(text) => Line::User {
            message: UserMessage {
                role: String::from("user"),
                content: Vec::from([TextBlock {
                    text,
                    kind: String::from("text"),
                }]),
            },
        },
        Outbound::Control { request_id, call } => Line::ControlRequest {
            request_id,
            request: match call {
                ControlCall::Initialize => Request::Initialize,
                ControlCall::Interrupt => Request::Interrupt,
                ControlCall::SetPermissionMode(mode) => {
                    Request::SetPermissionMode {
                        mode: String::from(mode.wire()),
                    }
                }
                ControlCall::GetSettings => Request::GetSettings,
                ControlCall::SetEffort(effort) => Request::ApplyFlagSettings {
                    settings: FlagSettings {
                        effort_level: String::from(effort.wire()),
                    },
                },
            },
        },
        Outbound::Permission { id, decision, open } => Line::ControlResponse {
            response: Reply::Success {
                request_id: id.0,
                response: serde_json::to_value(verdict(decision, open)?)?,
            },
        },
        Outbound::ControlError {
            request_id,
            message,
        } => Line::ControlResponse {
            response: Reply::Error {
                request_id,
                error: message,
            },
        },
    })
}

fn verdict(
    decision: PermissionDecision,
    open: Open,
) -> serde_json::Result<Verdict> {
    Ok(match decision {
        PermissionDecision::Allow => Verdict::Allow {
            updated_input: open.input,
            updated_permissions: None,
        },
        PermissionDecision::AllowForSession => Verdict::Allow {
            updated_input: open.input,
            updated_permissions: Some(
                open.grant.into_iter().map(|grant| grant.update).collect(),
            ),
        },
        PermissionDecision::Answer(answers) => Verdict::Allow {
            updated_input: answered(open.input, answers)?,
            updated_permissions: None,
        },
        PermissionDecision::Deny { message } => Verdict::Deny { message },
    })
}

/// `AskUserQuestion`'s input with `answers` added: each question's text to
/// its answer, the chosen labels joined with ", " as the CLI's own prompt
/// joins them.
fn answered(input: Value, given: Vec<Answer>) -> serde_json::Result<Value> {
    let prompts = questions(&input)?;
    if prompts.len() != given.len() {
        return Err(serde::de::Error::custom("wrong number of answers"));
    }
    let answers = prompts
        .into_iter()
        .zip(given)
        .map(|(asked, answer)| {
            (asked.question, Value::String(answer.0.join(", ")))
        })
        .collect::<serde_json::Map<String, Value>>();
    Ok(match input {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .chain([(String::from("answers"), Value::Object(answers))])
                .collect(),
        ),
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use proto::{
        Answer, Change, Effort, FileChange, ModelId, Question, QuestionOption,
        Selection, SubagentOutcome, ToolKind, ToolOutcome, TurnOutcome,
    };

    use super::{
        Block, ClaudeEvent, ControlCall, Inbound, MessageId, Open, Outbound,
        PermissionDecision, PermissionMode, PermissionRequestId, Role, encode,
        parse_line,
    };

    type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

    fn json(text: &str) -> serde_json::Result<Value> {
        serde_json::from_str(text)
    }

    fn sent(msg: Outbound) -> serde_json::Result<Value> {
        json(&encode(msg)?)
    }

    /// A `can_use_tool` request as the CLI prints it, read: what the actor
    /// emits, and what it keeps for the answer.
    fn asked(line: &str) -> TestResult<(ClaudeEvent, Open)> {
        match parse_line(line)? {
            Inbound::Permission { open, event, .. } => Ok((event, open)),
            other @ (Inbound::Event(_)
            | Inbound::ControlResponse { .. }
            | Inbound::UnsupportedControlRequest { .. }) => {
                Err(format!("not a permission request: {other:?}").into())
            }
        }
    }

    /// The `response` of our answer to the request on `line`.
    fn answered(
        line: &str,
        decision: PermissionDecision,
    ) -> TestResult<(ClaudeEvent, Value)> {
        let (event, open) = asked(line)?;
        let reply = sent(Outbound::Permission {
            id: PermissionRequestId(String::from("r1")),
            decision,
            open,
        })?;
        let response = reply
            .get("response")
            .and_then(|envelope| envelope.get("response"))
            .cloned()
            .ok_or("the reply has no response")?;
        Ok((event, response))
    }

    /// Recorded from Claude Code 2.1.316 on 2026-11-12.
    const BASH: &str = r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","display_name":"Bash","input":{"command":"touch t1.txt","description":"Create empty file t1.txt"},"description":"Create empty file t1.txt","permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"touch t1.txt"}],"behavior":"allow","destination":"localSettings"},{"type":"addDirectories","directories":["/tmp/s1-scratch"],"destination":"session"},{"type":"setMode","mode":"acceptEdits","destination":"session"}],"blocked_path":"/tmp/s1-scratch/t1.txt","tool_use_id":"toolu_1"}}"#;

    const WRITE: &str = r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Write","display_name":"Write","input":{"file_path":"/tmp/a.txt","content":"hi"},"description":"a.txt","permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}],"tool_use_id":"toolu_2"}}"#;

    #[test]
    fn message_boundaries_read_transcript_uuid_not_api_message_id()
    -> Result<(), Box<dyn std::error::Error>> {
        let cases = Vec::from([
            (
                r#"{"type":"assistant","uuid":"assistant-entry","message":{"id":"msg_api","content":[]}}"#,
                Some(MessageId(String::from("assistant-entry"))),
            ),
            (
                r#"{"type":"user","uuid":"user-entry","message":{"content":[]}}"#,
                Some(MessageId(String::from("user-entry"))),
            ),
            (
                r#"{"type":"assistant","message":{"id":"msg_api","content":[]}}"#,
                None,
            ),
            (
                r#"{"type":"assistant","uuid":"nested-entry","parent_tool_use_id":"toolu_agent","message":{"content":[]}}"#,
                None,
            ),
        ]);
        assert_eq!(
            cases
                .into_iter()
                .map(|(line, expected)| {
                    let actual = match parse_line(line)? {
                        Inbound::Event(ClaudeEvent::Message {
                            uuid, ..
                        }) => uuid,
                        Inbound::Event(
                            ClaudeEvent::Unknown(_)
                            | ClaudeEvent::UnidentifiedSubagentMessage { .. },
                        ) => None,
                        frame => {
                            assert_eq!(
                                format!("{frame:?}"),
                                "expected message boundary"
                            );
                            None
                        }
                    };
                    Ok::<_, Box<dyn std::error::Error>>((actual, expected))
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .all(|(actual, expected)| actual == expected)
                .then_some(()),
            Some(())
        );
        Ok(())
    }

    #[test]
    fn task_and_nested_frames_read_as_sub_agents() -> TestResult<()> {
        let events = [
            r#"{"type":"system","subtype":"task_started","task_id":"b1","tool_use_id":"toolu_1","description":"sleep","task_type":"local_bash","session_id":"s"}"#,
            r#"{"type":"system","subtype":"task_notification","task_id":"a1","tool_use_id":"toolu_2","status":"failed","output_file":"","summary":"","session_id":"s"}"#,
            r#"{"type":"system","subtype":"task_notification","task_id":"a2","status":"stopped","output_file":"","summary":"cut short","session_id":"s"}"#,
            r#"{"type":"stream_event","event":{"delta":{"type":"text_delta","text":"hi"}},"parent_tool_use_id":"toolu_2","session_id":"s"}"#,
            r#"{"type":"assistant","parent_tool_use_id":"toolu_2","agent_id":"a1","uuid":"u","message":{"content":[{"type":"text","text":"hi"}]},"session_id":"s"}"#,
        ]
        .into_iter()
        .map(|line| match parse_line(line)? {
            Inbound::Event(ClaudeEvent::Unknown(_)) => Ok(None),
            Inbound::Event(event) => Ok(Some(event)),
            other => Err(format!("not an event: {other:?}").into()),
        })
        .collect::<TestResult<Vec<Option<ClaudeEvent>>>>()?;
        assert_eq!(
            events,
            Vec::from([
                None,
                Some(ClaudeEvent::SubagentEnded {
                    agent: String::from("a1"),
                    outcome: SubagentOutcome::Failed,
                    summary: None,
                }),
                Some(ClaudeEvent::SubagentEnded {
                    agent: String::from("a2"),
                    outcome: SubagentOutcome::Stopped,
                    summary: Some(String::from("cut short")),
                }),
                None,
                Some(ClaudeEvent::SubagentMessage {
                    agent: String::from("a1"),
                    role: Role::Assistant,
                    content: Vec::from([Block::Text(String::from("hi"))]),
                }),
            ])
        );
        Ok(())
    }

    const ASK: &str = r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","display_name":"AskUserQuestion","input":{"questions":[{"question":"Which fruits do you like?","header":"Fruits","options":[{"label":"Apple","description":"Crisp"},{"label":"Pear","description":"Soft"}],"multiSelect":true},{"question":"Which color do you prefer?","header":"Color","options":[{"label":"Red","description":"Warm"},{"label":"Blue","description":"Cool"}],"multiSelect":false}]},"tool_use_id":"toolu_3","requires_user_interaction":true}}"#;

    #[test]
    fn repeated_question_text_is_refused_and_wrong_counts_fail()
    -> TestResult<()> {
        let line = ASK
            .replace("Which color do you prefer?", "Which fruits do you like?");
        assert!(matches!(
            parse_line(&line)?,
            Inbound::UnsupportedControlRequest { request_id, .. } if request_id == "r1"
        ));

        let (_, open) = asked(ASK)?;
        let reply = |answers| Outbound::Permission {
            id: PermissionRequestId(String::from("r1")),
            decision: PermissionDecision::Answer(answers),
            open: open.clone(),
        };
        let answer = Answer(Vec::from([String::from("Pear")]));
        assert!(sent(reply(Vec::from([answer.clone()]))).is_err());
        assert!(
            sent(reply(Vec::from([
                answer.clone(),
                answer.clone(),
                answer.clone()
            ])))
            .is_err()
        );
        assert!(sent(reply(Vec::from([answer.clone(), answer]))).is_ok());
        Ok(())
    }

    #[test]
    fn allow_echoes_the_proposed_input_as_updated_input() -> TestResult<()> {
        assert_eq!(
            answered(WRITE, PermissionDecision::Allow)?.1,
            json(
                r#"{"behavior": "allow", "updatedInput": {"file_path": "/tmp/a.txt", "content": "hi"}}"#
            )?
        );
        Ok(())
    }

    #[test]
    fn allow_for_session_adds_the_suggested_rules_for_the_session_only()
    -> TestResult<()> {
        let (event, response) =
            answered(BASH, PermissionDecision::AllowForSession)?;
        assert_eq!(
            event,
            ClaudeEvent::PermissionRequested {
                agent: None,
                id: PermissionRequestId(String::from("r1")),
                tool_name: String::from("Bash"),
                tool: ToolKind::Command {
                    command: String::from("touch t1.txt")
                },
                tool_use_id: String::from("toolu_1"),
                for_session: Some(String::from("Bash(touch t1.txt)")),
            }
        );
        assert_eq!(
            response,
            json(
                r#"{"behavior": "allow",
                "updatedInput": {"command": "touch t1.txt", "description": "Create empty file t1.txt"},
                "updatedPermissions": [{"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "touch t1.txt"}],
                    "behavior": "allow", "destination": "session"}]}"#
            )?
        );
        Ok(())
    }

    #[test]
    fn a_mixed_rule_list_offers_no_session_grant() -> TestResult<()> {
        let mut frame = json(BASH)?;
        frame["request"]["permission_suggestions"][0]["rules"] = json(
            r#"[{"toolName":"Bash","ruleContent":"touch t1.txt"},{"toolName":"Bash","scope":{"future":"wildcard"}}]"#,
        )?;
        let (event, response) =
            answered(&frame.to_string(), PermissionDecision::AllowForSession)?;
        assert!(matches!(
            event,
            ClaudeEvent::PermissionRequested {
                for_session: None,
                ..
            }
        ));
        assert_eq!(response.get("updatedPermissions"), Some(&json("[]")?));
        Ok(())
    }

    #[test]
    fn a_suggested_mode_is_no_session_grant() -> TestResult<()> {
        let (event, response) =
            answered(WRITE, PermissionDecision::AllowForSession)?;
        assert_eq!(
            (event, response.get("updatedPermissions")),
            (
                ClaudeEvent::PermissionRequested {
                    agent: None,
                    id: PermissionRequestId(String::from("r1")),
                    tool_name: String::from("Write"),
                    tool: ToolKind::FileChange {
                        files: Vec::from([FileChange {
                            path: String::from("/tmp/a.txt"),
                            change: Change::Write
                        }])
                    },
                    tool_use_id: String::from("toolu_2"),
                    for_session: None,
                },
                Some(&json("[]")?),
            )
        );
        Ok(())
    }

    fn option(label: &str, description: &str) -> QuestionOption {
        QuestionOption {
            label: String::from(label),
            description: String::from(description),
        }
    }

    #[test]
    fn ask_user_question_is_a_question_answered_in_its_input() -> TestResult<()>
    {
        let (event, response) = answered(
            ASK,
            PermissionDecision::Answer(Vec::from([
                Answer(Vec::from([
                    String::from("Apple"),
                    String::from("Pear"),
                ])),
                Answer(Vec::from([String::from("Green, really")])),
            ])),
        )?;
        assert_eq!(
            event,
            ClaudeEvent::QuestionAsked {
                agent: None,
                id: PermissionRequestId(String::from("r1")),
                questions: Vec::from([
                    Question {
                        header: String::from("Fruits"),
                        text: String::from("Which fruits do you like?"),
                        options: Vec::from([
                            option("Apple", "Crisp"),
                            option("Pear", "Soft")
                        ]),
                        selection: Selection::Many,
                    },
                    Question {
                        header: String::from("Color"),
                        text: String::from("Which color do you prefer?"),
                        options: Vec::from([
                            option("Red", "Warm"),
                            option("Blue", "Cool")
                        ]),
                        selection: Selection::One,
                    },
                ]),
                tool_use_id: String::from("toolu_3"),
            }
        );
        assert_eq!(
            (
                response.get("behavior"),
                response
                    .get("updatedInput")
                    .and_then(|input| input.get("answers"))
            ),
            (
                Some(&json(r#""allow""#)?),
                Some(&json(
                    r#"{"Which fruits do you like?": "Apple, Pear", "Which color do you prefer?": "Green, really"}"#
                )?)
            )
        );
        Ok(())
    }

    fn denied() -> TestResult<Value> {
        let (_, open) = asked(WRITE)?;
        Ok(sent(Outbound::Permission {
            id: PermissionRequestId(String::from("r2")),
            decision: PermissionDecision::Deny {
                message: String::from("not b.txt"),
            },
            open,
        })?)
    }

    #[test]
    fn denial_sends_the_reason() -> TestResult<()> {
        assert_eq!(
            denied()?,
            json(
                r#"{"type": "control_response", "response": {"subtype": "success", "request_id": "r2",
                "response": {"behavior": "deny", "message": "not b.txt"}}}"#
            )?
        );
        Ok(())
    }

    fn control(call: ControlCall) -> serde_json::Result<Value> {
        sent(Outbound::Control {
            request_id: String::from("req_7"),
            call,
        })
    }

    #[test]
    fn control_requests_match_the_sdk_envelope() -> TestResult<()> {
        assert_eq!(
            control(ControlCall::Interrupt)?,
            json(
                r#"{"type": "control_request", "request_id": "req_7", "request": {"subtype": "interrupt"}}"#
            )?
        );
        assert_eq!(
            control(ControlCall::SetPermissionMode(
                PermissionMode::AcceptEdits
            ))?,
            json(
                r#"{"type": "control_request", "request_id": "req_7",
                "request": {"subtype": "set_permission_mode", "mode": "acceptEdits"}}"#
            )?
        );
        // The SDK also sends `"hooks": null`; we omit it and the CLI accepts that.
        assert_eq!(
            control(ControlCall::Initialize)?,
            json(
                r#"{"type": "control_request", "request_id": "req_7", "request": {"subtype": "initialize"}}"#
            )?
        );
        Ok(())
    }

    #[test]
    fn user_text_is_sent_as_one_text_block() -> TestResult<()> {
        assert_eq!(
            sent(Outbound::User(String::from("create a.txt")))?,
            json(
                r#"{"type": "user", "message": {"role": "user",
                "content": [{"type": "text", "text": "create a.txt"}]}}"#
            )?
        );
        Ok(())
    }

    #[test]
    fn unsupported_control_request_gets_an_error_reply() -> TestResult<()> {
        assert_eq!(
            sent(Outbound::ControlError {
                request_id: String::from("cli_9"),
                message: String::from(
                    "unsupported control request subtype: hook_callback"
                ),
            })?,
            json(
                r#"{"type": "control_response", "response": {"subtype": "error", "request_id": "cli_9",
                "error": "unsupported control request subtype: hook_callback"}}"#
            )?
        );
        Ok(())
    }

    #[test]
    fn unknown_type_becomes_unknown_event_with_the_raw_json()
    -> Result<(), Box<dyn std::error::Error>> {
        let line = r#"{"type":"brand_new_frame","x":1}"#;
        assert_eq!(
            parse_line(line)?,
            Inbound::Event(ClaudeEvent::Unknown(json(
                r#"{"type": "brand_new_frame", "x": 1}"#
            )?))
        );
        Ok(())
    }

    #[test]
    fn unknown_control_subtype_still_yields_its_request_id()
    -> Result<(), Box<dyn std::error::Error>> {
        let line = r#"{"type":"control_request","request_id":"cli_3","request":{"subtype":"elicit_frobnicate","q":1}}"#;
        assert_eq!(
            parse_line(line)?,
            Inbound::UnsupportedControlRequest {
                request_id: String::from("cli_3"),
                subtype: String::from("elicit_frobnicate")
            }
        );
        Ok(())
    }

    #[test]
    fn malformed_can_use_tool_is_still_answered()
    -> Result<(), Box<dyn std::error::Error>> {
        let line = r#"{"type":"control_request","request_id":"cli_5","request":{"subtype":"can_use_tool"}}"#;
        assert_eq!(
            parse_line(line)?,
            Inbound::UnsupportedControlRequest {
                request_id: String::from("cli_5"),
                subtype: String::from("can_use_tool")
            }
        );
        Ok(())
    }

    #[test]
    fn can_use_tool_without_its_tool_use_id_is_refused()
    -> Result<(), Box<dyn std::error::Error>> {
        let line = r#"{"type":"control_request","request_id":"cli_6","request":{"subtype":"can_use_tool","tool_name":"Write","input":{"file_path":"a.txt"}}}"#;
        assert_eq!(
            parse_line(line)?,
            Inbound::UnsupportedControlRequest {
                request_id: String::from("cli_6"),
                subtype: String::from("can_use_tool without tool_use_id")
            }
        );
        Ok(())
    }

    #[test]
    fn hook_callback_is_unsupported_not_unknown()
    -> Result<(), Box<dyn std::error::Error>> {
        let line = r#"{"type":"control_request","request_id":"cli_4","request":{"subtype":"hook_callback","callback_id":"hook_0","input":{}}}"#;
        assert_eq!(
            parse_line(line)?,
            Inbound::UnsupportedControlRequest {
                request_id: String::from("cli_4"),
                subtype: String::from("hook_callback")
            }
        );
        Ok(())
    }

    struct ToolCase {
        name: String,
        input: &'static str,
        want: ToolKind,
    }

    fn other(name: &str, input: &'static str, detail: &str) -> ToolCase {
        ToolCase {
            name: String::from(name),
            input,
            want: ToolKind::Other {
                name: String::from(name),
                detail: String::from(detail),
            },
        }
    }

    fn tool_cases() -> [ToolCase; 12] {
        [
            other(
                "Agent",
                r#"{"description":"Review the diff","prompt":"..."}"#,
                "Review the diff",
            ),
            ToolCase {
                name: String::from("Bash"),
                input: r#"{"command":"ls -a"}"#,
                want: ToolKind::Command {
                    command: String::from("ls -a"),
                },
            },
            ToolCase {
                name: String::from("Write"),
                input: r#"{"file_path":"a.txt"}"#,
                want: ToolKind::FileChange {
                    files: Vec::from([FileChange {
                        path: String::from("a.txt"),
                        change: Change::Write,
                    }]),
                },
            },
            ToolCase {
                name: String::from("Edit"),
                input: r#"{"file_path":"b.txt"}"#,
                want: ToolKind::FileChange {
                    files: Vec::from([FileChange {
                        path: String::from("b.txt"),
                        change: Change::Edit,
                    }]),
                },
            },
            ToolCase {
                name: String::from("MultiEdit"),
                input: r#"{"file_path":"c.txt"}"#,
                want: ToolKind::FileChange {
                    files: Vec::from([FileChange {
                        path: String::from("c.txt"),
                        change: Change::Edit,
                    }]),
                },
            },
            other("Read", r#"{"file_path":"d.txt"}"#, "d.txt"),
            other("Grep", r#"{"pattern":"fn main"}"#, "fn main"),
            other("Glob", r#"{"pattern":"*.rs"}"#, "*.rs"),
            other(
                "WebFetch",
                r#"{"url":"https://example.com"}"#,
                "https://example.com",
            ),
            other(
                "Write",
                r#"{"content":"no path"}"#,
                r#"{"content":"no path"}"#,
            ),
            other("Bash", r#"{"command":7}"#, r#"{"command":7}"#),
            other(
                "TodoWrite",
                r#"{"todos":[{"content":"a","status":"later"}]}"#,
                r#"{"todos":[{"content":"a","status":"later"}]}"#,
            ),
        ]
    }

    #[test]
    fn tool_inputs_become_shared_tool_kinds()
    -> Result<(), Box<dyn std::error::Error>> {
        let cases = tool_cases();
        let (got, want) = cases.into_iter().map(|case| {
            let line = format!(r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"call","name":{},"input":{}}}]}}}}"#, serde_json::to_string(&case.name)?, json(case.input)?);
            let got = match parse_line(&line)? {
                Inbound::Event(ClaudeEvent::Message { content, .. }) => content.into_iter().find_map(|block| match block {
                    super::Block::ToolUse { tool, .. } => Some(tool),
                    super::Block::Text(_) | super::Block::Thinking(_) | super::Block::ToolResult { .. } | super::Block::Checklist(_) | super::Block::Other(_) => None,
                }),
                Inbound::Event(_) | Inbound::Permission { .. } | Inbound::ControlResponse { .. } | Inbound::UnsupportedControlRequest { .. } => None,
            };
            Ok((got, Some(case.want)))
        }).collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?.into_iter().unzip::<Option<ToolKind>, Option<ToolKind>, Vec<Option<ToolKind>>, Vec<Option<ToolKind>>>();
        assert_eq!(got, want);
        Ok(())
    }

    #[test]
    fn turn_results_distinguish_errors_from_interrupts()
    -> Result<(), Box<dyn std::error::Error>> {
        struct Case {
            is_error: String,
            reason: Option<String>,
            want: TurnOutcome,
        }
        let cases = [
            Case {
                is_error: String::from("null"),
                reason: None,
                want: TurnOutcome::Completed,
            },
            Case {
                is_error: String::from("false"),
                reason: Some(String::from("aborted_tools")),
                want: TurnOutcome::Completed,
            },
            Case {
                is_error: String::from("true"),
                reason: Some(String::from("aborted_tools")),
                want: TurnOutcome::Interrupted,
            },
            Case {
                is_error: String::from("true"),
                reason: Some(String::from("aborted_streaming")),
                want: TurnOutcome::Interrupted,
            },
            Case {
                is_error: String::from("true"),
                reason: None,
                want: TurnOutcome::Failed {
                    message: String::from("error_during_execution"),
                },
            },
            Case {
                is_error: String::from("true"),
                reason: Some(String::from("new_reason")),
                want: TurnOutcome::Failed {
                    message: String::from("error_during_execution"),
                },
            },
        ];
        let (got, want) = cases.into_iter().map(|case| {
            let line = format!(r#"{{"type":"result","subtype":"error_during_execution","session_id":"s","is_error":{},"terminal_reason":{}}}"#, case.is_error, serde_json::to_string(&case.reason)?);
            let got = match parse_line(&line)? {
                Inbound::Event(ClaudeEvent::TurnFinished { outcome, .. }) => Some(outcome),
                Inbound::Event(_) | Inbound::Permission { .. } | Inbound::ControlResponse { .. } | Inbound::UnsupportedControlRequest { .. } => None,
            };
            Ok((got, Some(case.want)))
        }).collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?.into_iter().unzip::<Option<TurnOutcome>, Option<TurnOutcome>, Vec<Option<TurnOutcome>>, Vec<Option<TurnOutcome>>>();
        assert_eq!(got, want);
        Ok(())
    }

    #[test]
    fn tool_results_use_the_shared_outcome_and_default_to_success()
    -> Result<(), Box<dyn std::error::Error>> {
        let cases = [
            ("", ToolOutcome::Succeeded),
            (r#","is_error":null"#, ToolOutcome::Succeeded),
            (r#","is_error":false"#, ToolOutcome::Succeeded),
            (r#","is_error":true"#, ToolOutcome::Failed),
        ];
        let (got, want) = cases.into_iter().map(|(field, want)| {
            let line = format!(r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"call"{field}}}]}}}}"#);
            let got = match parse_line(&line)? {
                Inbound::Event(ClaudeEvent::Message { content, .. }) => content.into_iter().find_map(|block| match block {
                    super::Block::ToolResult { outcome, .. } => Some(outcome),
                    super::Block::Text(_) | super::Block::Thinking(_) | super::Block::ToolUse { .. } | super::Block::Checklist(_) | super::Block::Other(_) => None,
                }),
                Inbound::Event(_) | Inbound::Permission { .. } | Inbound::ControlResponse { .. } | Inbound::UnsupportedControlRequest { .. } => None,
            };
            Ok((got, Some(want)))
        }).collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?.into_iter().unzip::<Option<ToolOutcome>, Option<ToolOutcome>, Vec<Option<ToolOutcome>>, Vec<Option<ToolOutcome>>>();
        assert_eq!(got, want);
        Ok(())
    }
    #[test]
    fn the_default_row_names_the_model_with_its_resolved_model()
    -> TestResult<()> {
        let listed = serde_json::from_str::<Value>(
            r#"{"models": [
                {"value": "default", "resolvedModel": "haiku-x", "displayName": "Default", "description": "d"},
                {"value": "opus", "resolvedModel": "opus-x", "displayName": "Opus", "description": "o", "supportedEffortLevels": ["low", "high"]},
                {"value": "haiku", "resolvedModel": "haiku-x", "displayName": "Haiku", "description": "h"},
                {"value": "custom", "displayName": "Custom", "description": "c"}
            ]}"#,
        )?;
        let models = super::models(&listed);
        assert_eq!(
            models
                .choices
                .iter()
                .map(|choice| (choice.id.0.as_str(), choice.efforts.clone()))
                .collect::<Vec<_>>(),
            Vec::from([
                ("opus", Vec::from([Effort::Low, Effort::High])),
                ("haiku", Vec::new()),
                ("custom", Vec::new()),
            ])
        );
        assert_eq!(models.default, Some(ModelId(String::from("haiku"))));
        let undefaulted = serde_json::from_str::<Value>(
            r#"{"models": [{"value": "custom", "displayName": "Custom", "description": "c"}]}"#,
        )?;
        assert_eq!(
            super::models(&undefaulted).default,
            None,
            "without a default row no model is the default"
        );
        Ok(())
    }
}
