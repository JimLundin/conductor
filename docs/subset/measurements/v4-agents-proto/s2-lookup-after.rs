impl Run {
    async fn on(&mut self, event: CodexEvent) -> Result<Progress> {
        match event {
            CodexEvent::Thread { thread, event } => {
                let turn = self.turns.get(&thread).with_context(|| {
                    format!("event for a thread with no turn running: {}", thread.0)
                })?;
                match event_turn(&event).filter(|got| got != &&turn.id) {
                    Some(got) => Err(anyhow::Error::msg(format!(
                        "event for turn {} during turn {}",
                        got.0, turn.id.0
                    ))),
                    None => Ok(()),
                }?;
                self.react(&thread, &event).await?;
                self.turns
                    .get_mut(&thread)
                    .context("turn left the run while reacting")?
                    .record(event);
                Ok(match self.running {
                    0 => Progress::Done,
                    _ => Progress::Playing,
                })
            }
            CodexEvent::Exited { status } => Err(anyhow::Error::msg(format!(
                "codex exited mid-turn: {}",
                status.describe()
            ))),
            CodexEvent::Unknown(_) => Ok(Progress::Playing),
        }
    }

    /// Does what the plan asks of an event before its turn records it.
    async fn react(&mut self, thread: &ThreadId, event: &ThreadEvent) -> Result<()> {
        match event {
            ThreadEvent::Started
            | ThreadEvent::Unknown(_)
            | ThreadEvent::TurnStarted { .. }
            | ThreadEvent::ApprovalCancelled { .. } => {}
            ThreadEvent::TurnCompleted { .. } => self.running = self.running.saturating_sub(1),
            ThreadEvent::AgentMessage { .. } | ThreadEvent::Item { .. } => {
                self.after_output(thread).await?;
            }
            ThreadEvent::ApprovalRequested { id, .. } => {
                let decision = match &self.plan {
                    Plan::Answer(policy) => policy.decision(),
                    Plan::InterruptOnFirstOutput => Decision::Accept,
                };
                self.live.server.respond(id.clone(), decision).await?;
            }
        }
        Ok(())
    }

    async fn after_output(&mut self, thread: &ThreadId) -> Result<()> {
        let turn = self
            .turns
            .get(thread)
            .context("turn left the run while reacting")?;
        if let (Plan::InterruptOnFirstOutput, None) = (&self.plan, &self.interrupted) {
            self.interrupted = Some(turn.thread.interrupt(turn.id.clone()).await);
        }
        Ok(())
    }
}

