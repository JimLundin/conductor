// codex/protocol.rs:612-627, 695-742, 762-817
/// The `config` override that sets the reasoning effort.
#[derive(Serialize)]
struct EffortConfig {
    model_reasoning_effort: ReasoningEffort,
}

fn effort_config(opts: &ThreadOptions) -> serde_json::Result<Option<Value>> {
    opts.effort
        .as_ref()
        .map(|effort| {
            serde_json::to_value(EffortConfig {
                model_reasoning_effort: ReasoningEffort(effort.clone()),
            })
        })
        .transpose()
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

/// The JSON-RPC `result` that answers an open request with `decision`.
pub(crate) fn answer(shape: &AnswerShape, decision: Decision) -> Result<Value, NotApplicable> {
    match (shape, decision) {
        (_, Decision::Raw(result)) => Ok(result),
        (AnswerShape::UserInput, _) => Err(NotApplicable {}),
        (AnswerShape::Command, decision) => encoded(command_answer(decision)),
        (AnswerShape::FileChange, decision) => encoded(file_change_answer(decision)),
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

/// Declining a permissions request grants an empty profile.
#[derive(Serialize)]
struct NoPermissions {
    permissions: Value,
}

fn encoded<T: Serialize>(answer: T) -> Result<Value, NotApplicable> {
    serde_json::to_value(answer).map_err(|_| NotApplicable {})
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
    id: RequestId,
    result: Value,
}

#[derive(Serialize)]
struct RejectFrame {
    error: RpcError,
    id: RequestId,
}

#[derive(Serialize)]
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
            method: String::from(methods::INITIALIZED),
        }),
        Outbound::Answer { rpc_id, result } => {
            serde_json::to_string(&AnswerFrame { id: rpc_id, result })
        }
        Outbound::Reject { rpc_id, message } => serde_json::to_string(&RejectFrame {
            error: RpcError {
                code: METHOD_NOT_FOUND,
                message,
            },
            id: rpc_id,
        }),
    }
}
