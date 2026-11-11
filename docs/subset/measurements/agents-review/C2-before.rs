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
