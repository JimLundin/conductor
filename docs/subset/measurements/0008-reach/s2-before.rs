// crates/agents/src/bin/s2.rs:99-101, 192-269
fn pushed(list: Vec<String>, item: String) -> Vec<String> {
    list.into_iter().chain(std::iter::once(item)).collect()
}
    async fn on(&mut self, event: CodexEvent) -> Result<Progress> {
        match event {
            CodexEvent::Thread { thread, event } => {
                let turn = self.turns.remove(&thread).with_context(|| {
                    format!("event for a thread with no turn running: {}", thread.0)
                })?;
                match event_turn(&event).map(|got| (got.0.cmp(&turn.id.0), got)) {
                    Some((Ordering::Less | Ordering::Greater, got)) => Err(anyhow::Error::msg(
                        format!("event for turn {} during turn {}", got.0, turn.id.0),
                    )),
                    Some((Ordering::Equal, _)) | None => Ok(()),
                }?;
                let turn = self.apply(turn, event).await?;
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

    async fn apply(&mut self, turn: Turn, event: ThreadEvent) -> Result<Turn> {
        match event {
            ThreadEvent::Started | ThreadEvent::Unknown(_) | ThreadEvent::TurnStarted { .. } => {
                Ok(turn)
            }
            ThreadEvent::TurnCompleted { outcome, .. } => {
                self.running = self.running.saturating_sub(1);
                Ok(Turn {
                    outcome: Some(outcome),
                    ..turn
                })
            }
            ThreadEvent::AgentMessage { text, .. } => {
                let text = match turn.text.as_str() {
                    "" => text,
                    earlier => format!("{earlier}\n{text}"),
                };
                self.after_output(&turn).await;
                Ok(Turn { text, ..turn })
            }
            ThreadEvent::Item { phase, item, .. } => {
                self.after_output(&turn).await;
                Ok(Turn {
                    items: pushed(turn.items, item_label(item, phase)),
                    ..turn
                })
            }
            ThreadEvent::ApprovalRequested {
                id, kind, options, ..
            } => {
                let label = approval_label(&id, kind, &options);
                let decision = match &self.plan {
                    Plan::Answer(policy) => policy.decision(),
                    Plan::InterruptOnFirstOutput => Decision::Accept,
                };
                self.live.server.respond(id.clone(), decision).await?;
                Ok(Turn {
                    approvals: pushed(turn.approvals, label),
                    answered: turn
                        .answered
                        .into_iter()
                        .chain(std::iter::once(id))
                        .collect(),
                    ..turn
                })
            }
            ThreadEvent::ApprovalCancelled { .. } => Ok(Turn {
                cancelled: turn.cancelled.saturating_add(1),
                ..turn
            }),
        }
    }
