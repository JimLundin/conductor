//! The stdio boundary. Each stdout line is parsed into the few frames we read
//! and mapped into our own types here, and every line we write is built here.
//! Frames and fields we do not model are ignored or passed on as `Unknown`.

use proto::{Answer, Question, QuestionOption, Selection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Exit;

/// A Claude session id, as printed in `system/init` and accepted by `--resume`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(pub String);

/// The CLI's `request_id` for a `can_use_tool` request.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PermissionRequestId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    Default,
    AcceptEdits,
    Plan,
    BypassPermissions,
    DontAsk,
    Auto,
}

impl PermissionMode {
    /// The name `--permission-mode` and `set_permission_mode` take.
    pub(crate) fn wire(&self) -> &str {
        match self {
            PermissionMode::Default => "default",
            PermissionMode::AcceptEdits => "acceptEdits",
            PermissionMode::Plan => "plan",
            PermissionMode::BypassPermissions => "bypassPermissions",
            PermissionMode::DontAsk => "dontAsk",
            PermissionMode::Auto => "auto",
        }
    }
}

/// What happens to the turn after a denied tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterDeny {
    /// The model sees the denial and carries on.
    Continue,
    /// The denial also ends the turn.
    Interrupt,
}

#[derive(Debug, Clone, PartialEq)]
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
        then: AfterDeny,
    },
}

/// The CLI's `is_error` on a tool result or a turn's `result`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Assistant,
    User,
}

/// What a tool call is about, read from the CLI's `input` by `tool_detail`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolDetail {
    Command(String),
    Path(String),
    /// A listed text field, or the compact JSON input of a tool not listed.
    Text(String),
}

/// The `input` field worth showing for each tool that has one, in its shape.
pub(crate) fn tool_detail(tool: &str, input: &Value) -> ToolDetail {
    let shown = match tool {
        "Bash" => field(input, "command").map(ToolDetail::Command),
        "Edit" | "Write" | "MultiEdit" | "Read" => field(input, "file_path").map(ToolDetail::Path),
        "Grep" | "Glob" => field(input, "pattern").map(ToolDetail::Text),
        "WebFetch" => field(input, "url").map(ToolDetail::Text),
        _ => None,
    };
    shown.unwrap_or_else(|| ToolDetail::Text(input.to_string()))
}

fn field(input: &Value, name: &str) -> Option<String> {
    input.get(name).and_then(Value::as_str).map(str::to_owned)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text(String),
    Thinking(String),
    ToolUse {
        id: String,
        name: String,
        detail: ToolDetail,
    },
    ToolResult {
        tool_use_id: String,
        content: Value,
        outcome: Outcome,
    },
    Other(Value),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClaudeEvent {
    /// The CLI read a user message and began its turn (`system/init`, which
    /// it prints once per turn with the same `session_id`).
    TurnStarted {
        session_id: SessionId,
    },
    Message {
        role: Role,
        content: Vec<Block>,
    },
    /// A piece of assistant text as it streams (`--include-partial-messages`).
    /// The whole text follows in a `Message`.
    StreamDelta {
        text: String,
    },
    PermissionRequested {
        id: PermissionRequestId,
        tool_name: String,
        detail: ToolDetail,
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
        outcome: Outcome,
        terminal_reason: Option<String>,
        session_id: SessionId,
    },
    Exited {
        status: Exit,
    },
    Unknown(Value),
}

/// What one stdout line means to the session actor.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Inbound {
    Event(ClaudeEvent),
    /// A `can_use_tool` request. The actor keeps `open` for the answer and
    /// emits `event`, a `PermissionRequested` or a `QuestionAsked`.
    Permission {
        id: PermissionRequestId,
        open: Open,
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
    /// Only `init` is read, which the CLI prints once per turn.
    System {
        subtype: SystemSubtype,
        session_id: String,
    },
    Assistant {
        message: Message,
    },
    User {
        message: Message,
    },
    StreamEvent {
        event: StreamEvent,
    },
    Result {
        subtype: String,
        is_error: Option<bool>,
        terminal_reason: Option<String>,
        session_id: String,
    },
    ControlRequest {
        request_id: String,
        request: Value,
    },
    ControlResponse {
        response: ControlResponse,
    },
    ControlCancelRequest {
        request_id: String,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum SystemSubtype {
    Init,
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
pub(crate) struct Open {
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

/// An entry of `permission_suggestions`. Rules are suggested for
/// `localSettings`, which would outlive the session, so a grant keeps only
/// their rules, verbatim, and sends them for `session`.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Suggestion {
    AddRules { rules: Value, behavior: Behavior },
    SetMode { mode: String },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum Behavior {
    Allow,
    Deny,
    Ask,
}

/// A permission rule, read only to name it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rule {
    tool_name: String,
    rule_content: Option<String>,
}

/// One entry of `updatedPermissions`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Update {
    AddRules {
        rules: Value,
        behavior: String,
        destination: String,
    },
    SetMode {
        mode: String,
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

const ASK_USER_QUESTION: &str = "AskUserQuestion";

impl Rule {
    /// The rule as the CLI writes it in settings: `Bash(git status)`.
    fn label(&self) -> String {
        match &self.rule_content {
            Some(content) => format!("{}({content})", self.tool_name),
            None => self.tool_name.clone(),
        }
    }
}

/// The session grant the CLI's suggestions offer: its allow rules, or else
/// the mode it suggests. Suggestions of other kinds are skipped.
fn grant(suggestions: Option<Value>) -> Option<Grant> {
    let suggestions = suggestions
        .and_then(|value| serde_json::from_value::<Vec<Value>>(value).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| serde_json::from_value::<Suggestion>(value).ok())
        .collect::<Vec<Suggestion>>();
    let rules = suggestions
        .iter()
        .filter_map(|suggestion| match suggestion {
            Suggestion::AddRules {
                rules,
                behavior: Behavior::Allow,
            } => serde_json::from_value::<Vec<Value>>(rules.clone()).ok(),
            Suggestion::AddRules { .. } | Suggestion::SetMode { .. } => None,
        })
        .flatten()
        .collect::<Vec<Value>>();
    let mode = suggestions
        .into_iter()
        .find_map(|suggestion| match suggestion {
            Suggestion::SetMode { mode } => Some(mode),
            Suggestion::AddRules { .. } => None,
        });
    match (rules.first(), mode) {
        (Some(_), _) => Some(Grant {
            label: rules
                .iter()
                .filter_map(|rule| serde_json::from_value::<Rule>(rule.clone()).ok())
                .map(|rule| rule.label())
                .collect::<Vec<String>>()
                .join(", "),
            update: Update::AddRules {
                rules: Value::Array(rules),
                behavior: String::from("allow"),
                destination: String::from("session"),
            },
        }),
        (None, Some(mode)) => Some(Grant {
            label: format!("{mode} mode"),
            update: Update::SetMode {
                mode,
                destination: String::from("session"),
            },
        }),
        (None, None) => None,
    }
}

fn questions(input: &Value) -> serde_json::Result<Vec<Asked>> {
    serde_json::from_value::<AskInput>(input.clone()).map(|ask| ask.questions)
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

pub(crate) fn parse_line(line: &str) -> serde_json::Result<Inbound> {
    let raw = serde_json::from_str::<Value>(line)?;
    Ok(match serde_json::from_value::<Frame>(raw.clone()) {
        Ok(frame) => inbound(frame, raw),
        Err(_) => Inbound::Event(ClaudeEvent::Unknown(raw)),
    })
}

fn inbound(frame: Frame, raw: Value) -> Inbound {
    match frame {
        Frame::System {
            subtype: SystemSubtype::Init,
            session_id,
        } => Inbound::Event(ClaudeEvent::TurnStarted {
            session_id: SessionId(session_id),
        }),
        Frame::Assistant { message } => message_event(Role::Assistant, message),
        Frame::User { message } => message_event(Role::User, message),
        Frame::StreamEvent {
            event:
                StreamEvent {
                    delta: Some(Delta::TextDelta { text }),
                },
        } => Inbound::Event(ClaudeEvent::StreamDelta { text }),
        Frame::StreamEvent {
            event: StreamEvent { delta: None },
        } => Inbound::Event(ClaudeEvent::Unknown(raw)),
        Frame::Result {
            subtype,
            is_error,
            terminal_reason,
            session_id,
        } => Inbound::Event(ClaudeEvent::TurnFinished {
            subtype,
            outcome: outcome(is_error),
            terminal_reason,
            session_id: SessionId(session_id),
        }),
        Frame::ControlRequest {
            request_id,
            request,
        } => control_request(request_id, request),
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

fn outcome(is_error: Option<bool>) -> Outcome {
    match is_error.unwrap_or(false).then_some(Outcome::Failed) {
        Some(failed) => failed,
        None => Outcome::Succeeded,
    }
}

/// A request we can't parse, unknown or malformed, still yields its
/// `request_id`, so it gets an error reply.
fn control_request(request_id: String, request: Value) -> Inbound {
    match serde_json::from_value::<ControlRequest>(request.clone()) {
        Ok(ControlRequest::CanUseTool {
            tool_name,
            input,
            tool_use_id: Some(tool_use_id),
            permission_suggestions,
        }) => {
            let id = PermissionRequestId(request_id);
            let (event, grant) = match (tool_name.as_str(), questions(&input)) {
                (ASK_USER_QUESTION, Ok(asked)) => (
                    ClaudeEvent::QuestionAsked {
                        id: id.clone(),
                        questions: asked
                            .into_iter()
                            .map(|asked| Question {
                                header: asked.header,
                                text: asked.question,
                                options: asked.options,
                                selection: match asked
                                    .multi_select
                                    .unwrap_or(false)
                                    .then_some(Selection::Many)
                                {
                                    Some(many) => many,
                                    None => Selection::One,
                                },
                            })
                            .collect(),
                        tool_use_id,
                    },
                    None,
                ),
                (_, _) => {
                    let grant = grant(permission_suggestions);
                    (
                        ClaudeEvent::PermissionRequested {
                            id: id.clone(),
                            detail: tool_detail(&tool_name, &input),
                            tool_name,
                            tool_use_id,
                            for_session: grant.as_ref().map(|grant| grant.label.clone()),
                        },
                        grant,
                    )
                }
            };
            Inbound::Permission {
                id,
                open: Open { input, grant },
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

fn message_event(role: Role, message: Message) -> Inbound {
    Inbound::Event(ClaudeEvent::Message {
        role,
        content: message.content.into_iter().map(block).collect(),
    })
}

fn block(raw: Value) -> Block {
    match serde_json::from_value::<ContentBlock>(raw.clone()) {
        Ok(ContentBlock::Text { text }) => Block::Text(text),
        Ok(ContentBlock::Thinking { thinking }) => Block::Thinking(thinking),
        Ok(ContentBlock::ToolUse { id, name, input }) => Block::ToolUse {
            detail: tool_detail(&name, &input),
            id,
            name,
        },
        Ok(ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        }) => Block::ToolResult {
            tool_use_id,
            content: content.unwrap_or(Value::Null),
            outcome: outcome(is_error),
        },
        Err(_) => Block::Other(raw),
    }
}

/// The CLI versions we have checked: from the version `s1` last passed against
/// to the newest one these types were checked against. Widen it only after
/// running `s1` against the new CLI.
const SUPPORTED_VERSIONS: &str = "2.1.316 to 2.1.322";

/// Why the CLI's version is not one we support.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UnsupportedVersion {
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

/// Checks the `claude_code_version` an `initialize` response reports.
pub(crate) fn check_version(initialize: Value) -> Result<(), UnsupportedVersion> {
    let found = serde_json::from_value::<InitializeResponse>(initialize)
        .map_err(|_| UnsupportedVersion::Unreported)?
        .claude_code_version;
    match found.as_str() {
        "2.1.316" | "2.1.317" | "2.1.318" | "2.1.319" | "2.1.320" | "2.1.321" | "2.1.322" => Ok(()),
        _ => Err(UnsupportedVersion::Outside {
            found,
            supported: String::from(SUPPORTED_VERSIONS),
        }),
    }
}

/// A control request we originate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControlCall {
    Initialize,
    Interrupt,
    SetPermissionMode(PermissionMode),
}

impl ControlCall {
    pub(crate) fn subtype(&self) -> &str {
        match self {
            Self::Initialize => "initialize",
            Self::Interrupt => "interrupt",
            Self::SetPermissionMode(_) => "set_permission_mode",
        }
    }
}

pub(crate) enum Outbound {
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
    },
    #[serde(rename = "allow")]
    AllowWithUpdates {
        #[serde(rename = "updatedInput")]
        updated_input: Value,
        #[serde(rename = "updatedPermissions")]
        updated_permissions: Vec<Update>,
    },
    Deny {
        message: String,
    },
    /// `interrupt` is only sent when it is `true`.
    #[serde(rename = "deny")]
    DenyAndInterrupt {
        message: String,
        interrupt: Value,
    },
}

pub(crate) fn encode(msg: Outbound) -> serde_json::Result<String> {
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
                ControlCall::SetPermissionMode(mode) => Request::SetPermissionMode {
                    mode: String::from(mode.wire()),
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

fn verdict(decision: PermissionDecision, open: Open) -> serde_json::Result<Verdict> {
    Ok(match decision {
        PermissionDecision::Allow => Verdict::Allow {
            updated_input: open.input,
        },
        PermissionDecision::AllowForSession => Verdict::AllowWithUpdates {
            updated_input: open.input,
            updated_permissions: open.grant.into_iter().map(|grant| grant.update).collect(),
        },
        PermissionDecision::Answer(answers) => Verdict::Allow {
            updated_input: answered(open.input, answers)?,
        },
        PermissionDecision::Deny {
            message,
            then: AfterDeny::Continue,
        } => Verdict::Deny { message },
        PermissionDecision::Deny {
            message,
            then: AfterDeny::Interrupt,
        } => Verdict::DenyAndInterrupt {
            message,
            interrupt: Value::Bool(true),
        },
    })
}

/// `AskUserQuestion`'s input with `answers` added: each question's text to
/// its answer, the chosen labels joined with ", " as the CLI's own prompt
/// joins them.
fn answered(input: Value, answers: Vec<Answer>) -> serde_json::Result<Value> {
    let answers = questions(&input)?
        .into_iter()
        .zip(answers)
        .map(|(asked, answer)| (asked.question, Value::String(answer.0.join(", "))))
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

    use proto::{Answer, Question, QuestionOption, Selection};

    use super::{
        AfterDeny, ClaudeEvent, ControlCall, Inbound, Open, Outbound, PermissionDecision,
        PermissionMode, PermissionRequestId, ToolDetail, encode, parse_line, tool_detail,
    };

    fn json(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    fn sent(msg: Outbound) -> Value {
        json(&encode(msg).unwrap())
    }

    /// A `can_use_tool` request as the CLI prints it, read: what the actor
    /// emits, and what it keeps for the answer.
    fn asked(line: &str) -> (ClaudeEvent, Open) {
        match parse_line(line).unwrap() {
            Inbound::Permission { open, event, .. } => Some((event, open)),
            Inbound::Event(_)
            | Inbound::ControlResponse { .. }
            | Inbound::UnsupportedControlRequest { .. } => None,
        }
        .unwrap()
    }

    /// The `response` of our answer to the request on `line`.
    fn answered(line: &str, decision: PermissionDecision) -> (ClaudeEvent, Value) {
        let (event, open) = asked(line);
        let sent = sent(Outbound::Permission {
            id: PermissionRequestId(String::from("r1")),
            decision,
            open,
        });
        let response = sent
            .get("response")
            .and_then(|reply| reply.get("response"))
            .cloned()
            .unwrap();
        (event, response)
    }

    /// Recorded from Claude Code 2.1.316 (`s1`, 2026-11-12).
    const BASH: &str = r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","display_name":"Bash","input":{"command":"touch t1.txt","description":"Create empty file t1.txt"},"description":"Create empty file t1.txt","permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"touch t1.txt"}],"behavior":"allow","destination":"localSettings"},{"type":"addDirectories","directories":["/tmp/s1-scratch"],"destination":"session"},{"type":"setMode","mode":"acceptEdits","destination":"session"}],"blocked_path":"/tmp/s1-scratch/t1.txt","tool_use_id":"toolu_1"}}"#;

    const WRITE: &str = r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Write","display_name":"Write","input":{"file_path":"/tmp/a.txt","content":"hi"},"description":"a.txt","permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}],"tool_use_id":"toolu_2"}}"#;

    const ASK: &str = r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","display_name":"AskUserQuestion","input":{"questions":[{"question":"Which fruits do you like?","header":"Fruits","options":[{"label":"Apple","description":"Crisp"},{"label":"Pear","description":"Soft"}],"multiSelect":true},{"question":"Which color do you prefer?","header":"Color","options":[{"label":"Red","description":"Warm"},{"label":"Blue","description":"Cool"}],"multiSelect":false}]},"tool_use_id":"toolu_3","requires_user_interaction":true}}"#;

    #[test]
    fn allow_echoes_the_proposed_input_as_updated_input() {
        assert_eq!(
            answered(WRITE, PermissionDecision::Allow).1,
            json(
                r#"{"behavior": "allow", "updatedInput": {"file_path": "/tmp/a.txt", "content": "hi"}}"#
            )
        );
    }

    #[test]
    fn allow_for_session_adds_the_suggested_rules_for_the_session_only() {
        let (event, response) = answered(BASH, PermissionDecision::AllowForSession);
        assert_eq!(
            event,
            ClaudeEvent::PermissionRequested {
                id: PermissionRequestId(String::from("r1")),
                tool_name: String::from("Bash"),
                detail: ToolDetail::Command(String::from("touch t1.txt")),
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
            )
        );
    }

    #[test]
    fn without_a_rule_the_session_grant_is_the_suggested_mode() {
        let (event, response) = answered(WRITE, PermissionDecision::AllowForSession);
        assert_eq!(
            event,
            ClaudeEvent::PermissionRequested {
                id: PermissionRequestId(String::from("r1")),
                tool_name: String::from("Write"),
                detail: ToolDetail::Path(String::from("/tmp/a.txt")),
                tool_use_id: String::from("toolu_2"),
                for_session: Some(String::from("acceptEdits mode")),
            }
        );
        assert_eq!(
            response.get("updatedPermissions"),
            Some(&json(
                r#"[{"type": "setMode", "mode": "acceptEdits", "destination": "session"}]"#
            ))
        );
    }

    fn option(label: &str, description: &str) -> QuestionOption {
        QuestionOption {
            label: String::from(label),
            description: String::from(description),
        }
    }

    #[test]
    fn ask_user_question_is_a_question_answered_in_its_input() {
        let (event, response) = answered(
            ASK,
            PermissionDecision::Answer(Vec::from([
                Answer(Vec::from([String::from("Apple"), String::from("Pear")])),
                Answer(Vec::from([String::from("Green, really")])),
            ])),
        );
        assert_eq!(
            event,
            ClaudeEvent::QuestionAsked {
                id: PermissionRequestId(String::from("r1")),
                questions: Vec::from([
                    Question {
                        header: String::from("Fruits"),
                        text: String::from("Which fruits do you like?"),
                        options: Vec::from([option("Apple", "Crisp"), option("Pear", "Soft")]),
                        selection: Selection::Many,
                    },
                    Question {
                        header: String::from("Color"),
                        text: String::from("Which color do you prefer?"),
                        options: Vec::from([option("Red", "Warm"), option("Blue", "Cool")]),
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
                Some(&json(r#""allow""#)),
                Some(&json(
                    r#"{"Which fruits do you like?": "Apple, Pear", "Which color do you prefer?": "Green, really"}"#
                ))
            )
        );
    }

    fn denied(then: AfterDeny) -> Value {
        let (_, open) = asked(WRITE);
        sent(Outbound::Permission {
            id: PermissionRequestId(String::from("r2")),
            decision: PermissionDecision::Deny {
                message: String::from("not b.txt"),
                then,
            },
            open,
        })
    }

    #[test]
    fn deny_sends_interrupt_only_when_set() {
        assert_eq!(
            denied(AfterDeny::Continue),
            json(
                r#"{"type": "control_response", "response": {"subtype": "success", "request_id": "r2",
                "response": {"behavior": "deny", "message": "not b.txt"}}}"#
            )
        );
        assert_eq!(
            denied(AfterDeny::Interrupt),
            json(
                r#"{"type": "control_response", "response": {"subtype": "success", "request_id": "r2",
                "response": {"behavior": "deny", "message": "not b.txt", "interrupt": true}}}"#
            )
        );
    }

    fn control(call: ControlCall) -> Value {
        sent(Outbound::Control {
            request_id: String::from("req_7"),
            call,
        })
    }

    #[test]
    fn control_requests_match_the_sdk_envelope() {
        assert_eq!(
            control(ControlCall::Interrupt),
            json(
                r#"{"type": "control_request", "request_id": "req_7", "request": {"subtype": "interrupt"}}"#
            )
        );
        assert_eq!(
            control(ControlCall::SetPermissionMode(PermissionMode::AcceptEdits)),
            json(
                r#"{"type": "control_request", "request_id": "req_7",
                "request": {"subtype": "set_permission_mode", "mode": "acceptEdits"}}"#
            )
        );
        // The SDK also sends `"hooks": null`; we omit it and the CLI accepts that.
        assert_eq!(
            control(ControlCall::Initialize),
            json(
                r#"{"type": "control_request", "request_id": "req_7", "request": {"subtype": "initialize"}}"#
            )
        );
    }

    #[test]
    fn user_text_is_sent_as_one_text_block() {
        assert_eq!(
            sent(Outbound::User(String::from("create a.txt"))),
            json(
                r#"{"type": "user", "message": {"role": "user",
                "content": [{"type": "text", "text": "create a.txt"}]}}"#
            )
        );
    }

    #[test]
    fn unsupported_control_request_gets_an_error_reply() {
        assert_eq!(
            sent(Outbound::ControlError {
                request_id: String::from("cli_9"),
                message: String::from("unsupported control request subtype: hook_callback"),
            }),
            json(
                r#"{"type": "control_response", "response": {"subtype": "error", "request_id": "cli_9",
                "error": "unsupported control request subtype: hook_callback"}}"#
            )
        );
    }

    #[test]
    fn tool_input_becomes_a_tool_detail() {
        let details = Vec::from([
            ("Bash", r#"{"command":"ls -a","description":"list"}"#),
            ("Grep", r#"{"pattern":"fn main"}"#),
            ("Write", r#"{"content":"no path"}"#),
            ("TodoWrite", r#"{"todos":[]}"#),
        ])
        .into_iter()
        .map(|(tool, input)| tool_detail(tool, &json(input)))
        .collect::<Vec<ToolDetail>>();
        assert_eq!(
            details,
            [
                ToolDetail::Command(String::from("ls -a")),
                ToolDetail::Text(String::from("fn main")),
                ToolDetail::Text(String::from(r#"{"content":"no path"}"#)),
                ToolDetail::Text(String::from(r#"{"todos":[]}"#)),
            ]
        );
    }

    #[test]
    fn unknown_type_becomes_unknown_event_with_the_raw_json() {
        let line = r#"{"type":"brand_new_frame","x":1}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Inbound::Event(ClaudeEvent::Unknown(json(
                r#"{"type": "brand_new_frame", "x": 1}"#
            )))
        );
    }

    #[test]
    fn unknown_control_subtype_still_yields_its_request_id() {
        let line = r#"{"type":"control_request","request_id":"cli_3","request":{"subtype":"elicit_frobnicate","q":1}}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Inbound::UnsupportedControlRequest {
                request_id: String::from("cli_3"),
                subtype: String::from("elicit_frobnicate")
            }
        );
    }

    #[test]
    fn malformed_can_use_tool_is_still_answered() {
        let line = r#"{"type":"control_request","request_id":"cli_5","request":{"subtype":"can_use_tool"}}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Inbound::UnsupportedControlRequest {
                request_id: String::from("cli_5"),
                subtype: String::from("can_use_tool")
            }
        );
    }

    #[test]
    fn can_use_tool_without_its_tool_use_id_is_refused() {
        let line = r#"{"type":"control_request","request_id":"cli_6","request":{"subtype":"can_use_tool","tool_name":"Write","input":{"file_path":"a.txt"}}}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Inbound::UnsupportedControlRequest {
                request_id: String::from("cli_6"),
                subtype: String::from("can_use_tool without tool_use_id")
            }
        );
    }

    #[test]
    fn hook_callback_is_unsupported_not_unknown() {
        let line = r#"{"type":"control_request","request_id":"cli_4","request":{"subtype":"hook_callback","callback_id":"hook_0","input":{}}}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Inbound::UnsupportedControlRequest {
                request_id: String::from("cli_4"),
                subtype: String::from("hook_callback")
            }
        );
    }
}
