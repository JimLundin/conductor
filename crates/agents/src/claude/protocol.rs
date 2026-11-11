//! The stdio boundary. Each stdout line is parsed with `claude-codes` and mapped
//! into our own types here, and every line we write is built here. Nothing from
//! `claude-codes` leaves this module.

use claude_codes::io::{
    ClaudeOutput, ContentBlock, ControlRequestMessage, ControlRequestPayload, ControlResponse,
    ControlResponseMessage, ControlResponsePayload, PermissionResult, SystemSubtype,
    ToolResultContent,
};
use claude_codes::{ClaudeInput, InitializeResponse};
use serde_json::Value;

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
    pub fn to_cli(self) -> claude_codes::PermissionMode {
        match self {
            PermissionMode::Default => claude_codes::PermissionMode::Default,
            PermissionMode::AcceptEdits => claude_codes::PermissionMode::AcceptEdits,
            PermissionMode::Plan => claude_codes::PermissionMode::Plan,
            PermissionMode::BypassPermissions => claude_codes::PermissionMode::BypassPermissions,
            PermissionMode::DontAsk => claude_codes::PermissionMode::DontAsk,
            PermissionMode::Auto => claude_codes::PermissionMode::Auto,
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

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text(String),
    Thinking(String),
    ToolUse {
        id: String,
        name: String,
        input: Value,
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
    SessionStarted {
        session_id: SessionId,
    },
    Message {
        role: Role,
        content: Vec<Block>,
    },
    PermissionRequested {
        id: PermissionRequestId,
        tool_name: String,
        input: Value,
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
        status: std::process::ExitStatus,
    },
    Unknown(Value),
}

/// What one stdout line means to the session actor.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Inbound {
    Event(ClaudeEvent),
    /// Answer to a control request we sent.
    ControlResponse {
        request_id: String,
        outcome: Result<Value, String>,
    },
    /// A control request we do not serve (hooks, SDK MCP servers, or a subtype
    /// `claude-codes` does not model). It must still get an error reply or the
    /// CLI waits forever.
    UnsupportedControlRequest {
        request_id: String,
        subtype: String,
    },
}

pub(crate) fn parse_line(line: &str) -> serde_json::Result<Inbound> {
    let raw = serde_json::from_str::<Value>(line)?;
    Ok(match serde_json::from_value::<ClaudeOutput>(raw.clone()) {
        Ok(output) => inbound(output, raw),
        Err(_) => Inbound::Event(ClaudeEvent::Unknown(raw)),
    })
}

fn inbound(output: ClaudeOutput, raw: Value) -> Inbound {
    match output {
        ClaudeOutput::System(msg) => match msg.subtype {
            SystemSubtype::Init => match msg.data.get("session_id").and_then(Value::as_str) {
                Some(id) => Inbound::Event(ClaudeEvent::SessionStarted {
                    session_id: SessionId(id.to_owned()),
                }),
                None => Inbound::Event(ClaudeEvent::Unknown(raw)),
            },
            _ => Inbound::Event(ClaudeEvent::Unknown(raw)),
        },
        ClaudeOutput::Assistant(msg) => Inbound::Event(ClaudeEvent::Message {
            role: Role::Assistant,
            content: msg.message.content.into_iter().map(block).collect(),
        }),
        ClaudeOutput::User(msg) => Inbound::Event(ClaudeEvent::Message {
            role: Role::User,
            content: msg.message.content.into_iter().map(block).collect(),
        }),
        ClaudeOutput::Result(msg) => Inbound::Event(ClaudeEvent::TurnFinished {
            subtype: msg.subtype.as_str().to_owned(),
            outcome: match msg.is_error.then_some(Outcome::Failed) {
                Some(failed) => failed,
                None => Outcome::Succeeded,
            },
            terminal_reason: msg.terminal_reason,
            session_id: SessionId(msg.session_id),
        }),
        ClaudeOutput::ControlRequest(req) => match req.request {
            ControlRequestPayload::CanUseTool(perm) => {
                Inbound::Event(ClaudeEvent::PermissionRequested {
                    id: PermissionRequestId(req.request_id),
                    tool_name: perm.tool_name,
                    input: perm.input,
                })
            }
            other => Inbound::UnsupportedControlRequest {
                request_id: req.request_id,
                subtype: other.subtype().to_owned(),
            },
        },
        ClaudeOutput::ControlResponse(resp) => match resp.response {
            ControlResponsePayload::Success {
                request_id,
                response,
            } => Inbound::ControlResponse {
                request_id,
                outcome: Ok(response.unwrap_or(Value::Null)),
            },
            ControlResponsePayload::Error { request_id, error } => Inbound::ControlResponse {
                request_id,
                outcome: Err(error),
            },
        },
        ClaudeOutput::ControlCancelRequest(cancel) => {
            Inbound::Event(ClaudeEvent::PermissionCancelled {
                id: PermissionRequestId(cancel.request_id),
            })
        }
        _ => Inbound::Event(ClaudeEvent::Unknown(raw)),
    }
}

fn block(b: ContentBlock) -> Block {
    match b {
        ContentBlock::Text(t) => Block::Text(t.text),
        ContentBlock::Thinking(t) => Block::Thinking(t.thinking),
        ContentBlock::ToolUse(t) => Block::ToolUse {
            id: t.id,
            name: t.name,
            input: t.input,
        },
        ContentBlock::ToolResult(t) => Block::ToolResult {
            tool_use_id: t.tool_use_id,
            content: match t.content {
                None => Value::Null,
                Some(ToolResultContent::Text(s)) => Value::String(s),
                Some(ToolResultContent::Structured(v)) => Value::Array(v),
            },
            outcome: match t.is_error.unwrap_or(false).then_some(Outcome::Failed) {
                Some(failed) => failed,
                None => Outcome::Succeeded,
            },
        },
        other => Block::Other(serde_json::to_value(other).unwrap_or(Value::Null)),
    }
}

/// A CLI version outside the range `claude-codes` supports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnsupportedVersion {
    pub(crate) found: String,
    pub(crate) supported: String,
}

/// Checks the `claude_code_version` an `initialize` response reports.
pub(crate) fn check_version(initialize: Value) -> Result<(), UnsupportedVersion> {
    let found = serde_json::from_value::<InitializeResponse>(initialize)
        .map(|r| r.claude_code_version)
        .unwrap_or_else(|_| String::from("<missing>"));
    claude_codes::check_cli_version(&found)
        .map(drop)
        .map_err(|e| UnsupportedVersion {
            found: e.found,
            supported: claude_codes::version::supported_cli_versions(),
        })
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

pub(crate) fn encode(msg: Outbound) -> serde_json::Result<String> {
    match msg {
        Outbound::User(text) => {
            serde_json::to_string(&ClaudeInput::user_message_without_session(text))
        }
        Outbound::Control {
            request_id,
            call: ControlCall::Initialize,
        } => serde_json::to_string(&ControlRequestMessage::initialize(request_id)),
        Outbound::Control {
            request_id,
            call: ControlCall::Interrupt,
        } => serde_json::to_string(&ControlRequestMessage::interrupt(request_id)),
        Outbound::Control {
            request_id,
            call: ControlCall::SetPermissionMode(mode),
        } => serde_json::to_string(&ControlRequestMessage::set_permission_mode(
            request_id,
            mode.to_cli(),
        )),
        Outbound::Permission {
            id,
            decision,
            input,
        } => serde_json::to_string(&ControlResponseMessage::from(ControlResponse::from_result(
            &id.0,
            permission_result(decision, input),
        ))),
        Outbound::ControlError {
            request_id,
            message,
        } => serde_json::to_string(&ControlResponseMessage::from(ControlResponse::error(
            &request_id,
            message,
        ))),
    }
}

fn permission_result(decision: PermissionDecision, input: Value) -> PermissionResult {
    match decision {
        PermissionDecision::Allow => PermissionResult::allow(input),
        PermissionDecision::Deny {
            message,
            then: AfterDeny::Continue,
        } => PermissionResult::Deny {
            message,
            interrupt: false,
        },
        PermissionDecision::Deny {
            message,
            then: AfterDeny::Interrupt,
        } => PermissionResult::Deny {
            message,
            interrupt: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{
        AfterDeny, ClaudeEvent, ControlCall, Inbound, Outbound, PermissionDecision, PermissionMode,
        PermissionRequestId, encode, parse_line,
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
        // The SDK also sends `"hooks": null`; claude-codes omits it and the CLI accepts that.
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
