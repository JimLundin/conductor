pub enum ToolOutcome {
    Succeeded,
    Failed,
    Declined,
}

/// Codex's item `status` strings and what they mean for a tool call.
fn codex_outcome(status: &str) -> Option<ToolOutcome> {
    match status {
        "completed" => Some(ToolOutcome::Succeeded),
        "failed" => Some(ToolOutcome::Failed),
        "declined" => Some(ToolOutcome::Declined),
        _ => None,
    }
}
