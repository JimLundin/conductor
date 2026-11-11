pub enum ToolOutcome {
    Succeeded,
    Failed,
    Declined,
}

/// Codex's item `status` strings and what they mean for a tool call.
const CODEX_OUTCOMES: &[CodexOutcome] = &[
    CodexOutcome { status: "completed", outcome: ToolOutcome::Succeeded },
    CodexOutcome { status: "failed", outcome: ToolOutcome::Failed },
    CodexOutcome { status: "declined", outcome: ToolOutcome::Declined },
];

struct CodexOutcome {
    status: &'static str,
    outcome: ToolOutcome,
}

fn codex_outcome(status: &str) -> Option<&ToolOutcome> {
    CODEX_OUTCOMES
        .iter()
        .find(|row| row.status == status)
        .map(|row| &row.outcome)
}
