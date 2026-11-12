//! The stdio boundary. Each stdout line is parsed into the few frames we read
//! and mapped into our own types here, and every line we write is built here.
//! Frames and fields we do not model are ignored or passed on as `Unknown`.

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
    /// A `can_use_tool` request. The actor keeps `input`, which `allow`
    /// echoes, and emits `PermissionRequested`.
    Permission {
        id: PermissionRequestId,
        tool_name: String,
        input: Value,
        tool_use_id: String,
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
    },
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
        }) => Inbound::Permission {
            id: PermissionRequestId(request_id),
            tool_name,
            input,
            tool_use_id,
        },
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
        /// The CLI's proposed input; `allow` echoes it back as `updatedInput`.
        input: Value,
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
        Outbound::Permission {
            id,
            decision,
            input,
        } => Line::ControlResponse {
            response: Reply::Success {
                request_id: id.0,
                response: serde_json::to_value(verdict(decision, input))?,
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

fn verdict(decision: PermissionDecision, input: Value) -> Verdict {
    match decision {
        PermissionDecision::Allow => Verdict::Allow {
            updated_input: input,
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
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{
        AfterDeny, ClaudeEvent, ControlCall, Inbound, Outbound, PermissionDecision, PermissionMode,
        PermissionRequestId, ToolDetail, encode, parse_line, tool_detail,
    };

    fn json(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    fn sent(msg: Outbound) -> Value {
        json(&encode(msg).unwrap())
    }

    #[test]
    fn allow_echoes_the_proposed_input_as_updated_input() {
        let line = sent(Outbound::Permission {
            id: PermissionRequestId(String::from("r1")),
            decision: PermissionDecision::Allow,
            input: json(r#"{"file_path": "/tmp/a.txt", "content": "hi"}"#),
        });
        assert_eq!(
            line,
            json(
                r#"{"type": "control_response", "response": {"subtype": "success", "request_id": "r1",
                "response": {"behavior": "allow", "updatedInput": {"file_path": "/tmp/a.txt", "content": "hi"}}}}"#
            )
        );
    }

    fn denied(then: AfterDeny) -> Value {
        sent(Outbound::Permission {
            id: PermissionRequestId(String::from("r2")),
            decision: PermissionDecision::Deny {
                message: String::from("not b.txt"),
                then,
            },
            input: json("{}"),
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
