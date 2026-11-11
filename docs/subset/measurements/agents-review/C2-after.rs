fn permission_result(decision: PermissionDecision, input: Value) -> PermissionResult {
    match decision {
        PermissionDecision::Allow => PermissionResult::allow(input),
        PermissionDecision::Deny { message, then } => PermissionResult::Deny {
            message,
            interrupt: match then {
                AfterDeny::Continue => false,
                AfterDeny::Interrupt => true,
            },
        },
    }
}
