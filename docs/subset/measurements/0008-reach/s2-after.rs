// s2.rs Run::on and Run::apply, with &mut Turn (a nested record of loop state)
    async fn on(&mut self, event: CodexEvent) -> Result<Progress> {
        match event {
            CodexEvent::Thread { thread, event } => {
                let mut turn = self.turns.remove(&thread).with_context(|| {
                    format!("event for a thread with no turn running: {}", thread.0)
                })?;
                match event_turn(&event).map(|got| (got.0.cmp(&turn.id.0), got)) {
                    Some((Ordering::Less | Ordering::Greater, got)) => Err(anyhow::Error::msg(
                        format!("event for turn {} during turn {}", got.0, turn.id.0),
                    )),
                    Some((Ordering::Equal, _)) | None => Ok(()),
                }?;
                self.apply(&mut turn, event).await?;
                self.turns.insert(thread, turn);
                Ok(match self.running {
                    0 => Progress::Done,
                    _ => Progress::Playing,
                })
            }
            CodexEvent::Exited { status } => Err(anyhow::Error::msg(format!(
                "codex exited mid-turn: {status}"
            ))),
            CodexEvent::Unknown(_) => Ok(Progress::Playing),
        }
    }

    async fn apply(&mut self, turn: &mut Turn, event: ThreadEvent) -> Result<()> {
        match event {
            ThreadEvent::Started | ThreadEvent::Unknown(_) | ThreadEvent::TurnStarted { .. } => {}
            ThreadEvent::TurnCompleted { outcome, .. } => {
                self.running = self.running.saturating_sub(1);
                turn.outcome = Some(outcome);
            }
            ThreadEvent::AgentMessage { text, .. } => {
                turn.text = match turn.text.as_str() {
                    "" => text,
                    earlier => format!("{earlier}\n{text}"),
                };
                self.after_output(turn).await;
            }
            ThreadEvent::Item { phase, item, .. } => {
                self.after_output(turn).await;
                turn.items.push(item_label(item, phase));
            }
            ThreadEvent::ApprovalRequested {
                id, kind, options, ..
            } => {
                turn.approvals.push(approval_label(&id, kind, &options));
                let decision = match &self.plan {
                    Plan::Answer(policy) => policy.decision(),
                    Plan::InterruptOnFirstOutput => Decision::Accept,
                };
                self.live.server.respond(id.clone(), decision).await?;
                turn.answered.push(id);
            }
            ThreadEvent::ApprovalCancelled { .. } => {
                turn.cancelled = turn.cancelled.saturating_add(1);
            }
        }
        Ok(())
    }
