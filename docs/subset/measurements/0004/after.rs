fn params(&self) -> serde_json::Result<Value> {
    match self {
        Self::ThreadStart(opts) => serde_json::to_value(ThreadStartParams {
            approval_policy: Some(approval_policy(opts.approval_policy)),
            config: effort_config(opts),
            cwd: Some(opts.cwd.display().to_string()),
            model: opts.model.clone(),
            sandbox: Some(sandbox(opts.sandbox)),
            ..Default::default()
        }),
        Self::TurnStart { thread, text } => serde_json::to_value(TurnStartParams {
            input: text_input(text),
            thread_id: thread.0.clone(),
            ..Default::default()
        }),
        Self::TurnSteer { thread, expected_turn, text } => serde_json::to_value(TurnSteerParams {
            expected_turn_id: expected_turn.0.clone(),
            input: text_input(text),
            thread_id: thread.0.clone(),
            ..Default::default()
        }),
    }
}
