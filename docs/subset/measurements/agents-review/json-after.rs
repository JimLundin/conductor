// codex/protocol.rs:612-627, 695-742, 762-817 with `serde_json::json!` allowed
fn effort_config(opts: &ThreadOptions) -> Option<Value> {
    opts.effort
        .as_ref()
        .map(|effort| json!({"model_reasoning_effort": ReasoningEffort(effort.clone())}))
}

/// The JSON-RPC `result` that answers an open request with `decision`.
pub(crate) fn answer(shape: &AnswerShape, decision: Decision) -> Result<Value, NotApplicable> {
    match (shape, decision) {
        (_, Decision::Raw(result)) => Ok(result),
        (AnswerShape::UserInput, _) => Err(NotApplicable {}),
        (AnswerShape::Command, decision) => encoded(command_answer(decision)),
        (AnswerShape::FileChange, decision) => encoded(file_change_answer(decision)),
        (AnswerShape::Permissions(requested), Decision::Accept) => {
            Ok(json!({"permissions": requested, "scope": "turn"}))
        }
        (AnswerShape::Permissions(requested), Decision::AcceptForSession) => {
            Ok(json!({"permissions": requested, "scope": "session"}))
        }
        (AnswerShape::Permissions(_), Decision::Decline | Decision::Cancel) => {
            Ok(json!({"permissions": {}}))
        }
    }
}

fn encoded<T: Serialize>(answer: T) -> Result<Value, NotApplicable> {
    serde_json::to_value(answer).map_err(|_| NotApplicable {})
}

pub(crate) fn encode(msg: Outbound) -> serde_json::Result<String> {
    let frame = match msg {
        Outbound::Request { id, call } => {
            json!({"id": id, "method": call.method(), "params": call.params()?})
        }
        Outbound::Initialized => json!({"method": methods::INITIALIZED}),
        Outbound::Answer { rpc_id, result } => json!({"id": rpc_id, "result": result}),
        Outbound::Reject { rpc_id, message } => {
            json!({"id": rpc_id, "error": {"code": -32601, "message": message}})
        }
    };
    serde_json::to_string(&frame)
}
