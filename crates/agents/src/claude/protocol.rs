//! The stdio boundary. Each stdout line is parsed with `claude-codes` and mapped
//! into our own types here, and every line we write is built here. Nothing from
//! `claude-codes` leaves this module.

use claude_codes::ClaudeInput;
use claude_codes::io::{
    ClaudeOutput, ContentBlock, ControlRequestMessage, ControlRequestPayload, ControlResponse,
    ControlResponseMessage, ControlResponsePayload, PermissionResult, SystemSubtype,
    ToolResultContent,
};
use serde_json::{Value, json};

/// A Claude session id, as printed in `system/init` and accepted by `--resume`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(pub String);

/// The CLI's `request_id` for a `can_use_tool` request.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PermissionRequestId(pub String);

impl std::fmt::Display for PermissionRequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

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
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            Self::Plan => "plan",
            Self::BypassPermissions => "bypassPermissions",
            Self::DontAsk => "dontAsk",
            Self::Auto => "auto",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PermissionDecision {
    /// Run the tool with the input the CLI proposed.
    Allow,
    /// `interrupt: true` also ends the turn.
    Deny { message: String, interrupt: bool },
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
        is_error: bool,
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
    /// `subtype` is the CLI's result subtype. An interrupted turn reports
    /// `error_during_execution` with `terminal_reason` `aborted_streaming`.
    TurnFinished {
        subtype: String,
        is_error: bool,
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
    /// `claude-codes` cannot parse). It must still get an error reply or the
    /// CLI waits forever.
    UnsupportedControlRequest {
        request_id: String,
        subtype: String,
    },
}

pub(crate) fn parse_line(line: &str) -> serde_json::Result<Inbound> {
    let raw: Value = serde_json::from_str(line)?;
    let Ok(output) = serde_json::from_value::<ClaudeOutput>(raw.clone()) else {
        return Ok(unparsed(raw));
    };
    Ok(match output {
        ClaudeOutput::System(msg) if msg.subtype == SystemSubtype::Init => {
            match msg.data.get("session_id").and_then(Value::as_str) {
                Some(id) => Inbound::Event(ClaudeEvent::SessionStarted {
                    session_id: SessionId(id.to_owned()),
                }),
                None => Inbound::Event(ClaudeEvent::Unknown(raw)),
            }
        }
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
            is_error: msg.is_error,
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
            _ => Inbound::UnsupportedControlRequest {
                request_id: req.request_id,
                subtype: subtype_of(&raw),
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
    })
}

/// `claude-codes` rejects unknown `type`s and unknown control subtypes outright.
/// A control request still needs a reply, so recover its id from the raw JSON.
fn unparsed(raw: Value) -> Inbound {
    let request_id = raw.get("request_id").and_then(Value::as_str);
    match (raw.get("type").and_then(Value::as_str), request_id) {
        (Some("control_request"), Some(id)) => Inbound::UnsupportedControlRequest {
            request_id: id.to_owned(),
            subtype: subtype_of(&raw),
        },
        _ => Inbound::Event(ClaudeEvent::Unknown(raw)),
    }
}

fn subtype_of(raw: &Value) -> String {
    raw.pointer("/request/subtype")
        .and_then(Value::as_str)
        .unwrap_or("<missing>")
        .to_owned()
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
            is_error: t.is_error.unwrap_or(false),
        },
        other => Block::Other(serde_json::to_value(other).unwrap_or(Value::Null)),
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
    pub(crate) fn subtype(self) -> &'static str {
        match self {
            Self::Initialize => "initialize",
            Self::Interrupt => "interrupt",
            Self::SetPermissionMode(_) => "set_permission_mode",
        }
    }
}

pub(crate) enum Outbound<'a> {
    User(&'a str),
    Control {
        request_id: &'a str,
        call: ControlCall,
    },
    Permission {
        id: &'a PermissionRequestId,
        decision: PermissionDecision,
        /// The CLI's proposed input; `allow` echoes it back as `updatedInput`.
        input: Value,
    },
    /// Reply to a control request we do not serve.
    ControlError {
        request_id: &'a str,
        message: &'a str,
    },
}

pub(crate) fn encode(msg: Outbound<'_>) -> serde_json::Result<String> {
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
        // claude-codes has no `set_permission_mode` payload; see docs/upstream-claude-codes.md.
        Outbound::Control {
            request_id,
            call: ControlCall::SetPermissionMode(mode),
        } => serde_json::to_string(&json!({
            "type": "control_request",
            "request_id": request_id,
            "request": {"subtype": "set_permission_mode", "mode": mode.as_str()},
        })),
        Outbound::Permission {
            id,
            decision,
            input,
        } => {
            let result = match decision {
                PermissionDecision::Allow => PermissionResult::allow(input),
                PermissionDecision::Deny { message, interrupt } => {
                    PermissionResult::Deny { message, interrupt }
                }
            };
            let response: ControlResponseMessage =
                ControlResponse::from_result(&id.0, result).into();
            serde_json::to_string(&response)
        }
        Outbound::ControlError {
            request_id,
            message,
        } => {
            let response: ControlResponseMessage =
                ControlResponse::error(request_id, message).into();
            serde_json::to_string(&response)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(msg: Outbound<'_>) -> Value {
        serde_json::from_str(&encode(msg).unwrap()).unwrap()
    }

    #[test]
    fn allow_echoes_the_proposed_input_as_updated_input() {
        let id = PermissionRequestId("r1".into());
        let line = sent(Outbound::Permission {
            id: &id,
            decision: PermissionDecision::Allow,
            input: json!({"file_path": "/tmp/a.txt", "content": "hi"}),
        });
        assert_eq!(
            line,
            json!({"type": "control_response", "response": {"subtype": "success", "request_id": "r1",
                "response": {"behavior": "allow", "updatedInput": {"file_path": "/tmp/a.txt", "content": "hi"}}}})
        );
    }

    #[test]
    fn deny_sends_interrupt_only_when_set() {
        let id = PermissionRequestId("r2".into());
        let deny = |interrupt| {
            sent(Outbound::Permission {
                id: &id,
                decision: PermissionDecision::Deny {
                    message: "not b.txt".into(),
                    interrupt,
                },
                input: json!({}),
            })
        };
        assert_eq!(
            deny(false),
            json!({"type": "control_response", "response": {"subtype": "success", "request_id": "r2",
                "response": {"behavior": "deny", "message": "not b.txt"}}})
        );
        assert_eq!(
            deny(true),
            json!({"type": "control_response", "response": {"subtype": "success", "request_id": "r2",
                "response": {"behavior": "deny", "message": "not b.txt", "interrupt": true}}})
        );
    }

    #[test]
    fn control_requests_match_the_sdk_envelope() {
        let control = |call| {
            sent(Outbound::Control {
                request_id: "req_7",
                call,
            })
        };
        assert_eq!(
            control(ControlCall::Interrupt),
            json!({"type": "control_request", "request_id": "req_7", "request": {"subtype": "interrupt"}})
        );
        assert_eq!(
            control(ControlCall::SetPermissionMode(PermissionMode::AcceptEdits)),
            json!({"type": "control_request", "request_id": "req_7",
                "request": {"subtype": "set_permission_mode", "mode": "acceptEdits"}})
        );
        // The SDK also sends `"hooks": null`; claude-codes omits it and the CLI accepts that.
        assert_eq!(
            control(ControlCall::Initialize),
            json!({"type": "control_request", "request_id": "req_7", "request": {"subtype": "initialize"}})
        );
    }

    #[test]
    fn user_text_is_sent_as_one_text_block() {
        assert_eq!(
            sent(Outbound::User("create a.txt")),
            json!({"type": "user", "message": {"role": "user",
                "content": [{"type": "text", "text": "create a.txt"}]}})
        );
    }

    #[test]
    fn unsupported_control_request_gets_an_error_reply() {
        assert_eq!(
            sent(Outbound::ControlError {
                request_id: "cli_9",
                message: "unsupported control request subtype: hook_callback",
            }),
            json!({"type": "control_response", "response": {"subtype": "error", "request_id": "cli_9",
                "error": "unsupported control request subtype: hook_callback"}})
        );
    }

    #[test]
    fn unknown_type_becomes_unknown_event_with_the_raw_json() {
        let line = r#"{"type":"brand_new_frame","x":1}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Inbound::Event(ClaudeEvent::Unknown(
                json!({"type": "brand_new_frame", "x": 1})
            ))
        );
    }

    #[test]
    fn unknown_control_subtype_still_yields_its_request_id() {
        let line = r#"{"type":"control_request","request_id":"cli_3","request":{"subtype":"elicit_frobnicate","q":1}}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Inbound::UnsupportedControlRequest {
                request_id: "cli_3".into(),
                subtype: "elicit_frobnicate".into()
            }
        );
    }

    #[test]
    fn hook_callback_is_unsupported_not_unknown() {
        let line = r#"{"type":"control_request","request_id":"cli_4","request":{"subtype":"hook_callback","callback_id":"hook_0","input":{}}}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Inbound::UnsupportedControlRequest {
                request_id: "cli_4".into(),
                subtype: "hook_callback".into()
            }
        );
    }
}
