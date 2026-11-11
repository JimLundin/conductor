// src/codex/server.rs:374-435 (rewritten)
    async fn on_line(&mut self, line: &str) {
        match self.stdout_log.as_mut().map(|log| writeln!(log, "{line}")) {
            Some(Ok(())) => {}
            Some(Err(e)) => tracing::warn!("writing stdout log: {e}"),
            None => {}
        }
        let inbound = match parse_line(line) {
            Ok(inbound) => inbound,
            Err(e) => {
                tracing::warn!("codex printed a non-JSON line ({e}): {line}");
                Inbound::Event(CodexEvent::Unknown(Value::String(line.to_owned())))
            }
        };
        match inbound {
            Inbound::Event(event) => self.emit(event).await,
            Inbound::Response { id, outcome } => self.on_response(id, outcome).await,
            Inbound::Approval {
                rpc_id,
                thread,
                shape,
                event,
            } => {
                self.open.insert(
                    ApprovalId::of(&rpc_id),
                    Open {
                        rpc_id,
                        thread: thread.clone(),
                        shape,
                    },
                );
                self.emit(CodexEvent::Thread { thread, event }).await;
            }
            Inbound::Resolved { thread, rpc_id } => {
                let id = ApprovalId::of(&rpc_id);
                // Codex also resolves requests we answered; only an unanswered
                // one means Codex withdrew it.
                match self.open.remove(&id) {
                    Some(_) => {
                        self.emit(CodexEvent::Thread {
                            thread,
                            event: ThreadEvent::ApprovalCancelled { id },
                        })
                        .await;
                    }
                    None => {}
                }
            }
            Inbound::Unsupported {
                rpc_id,
                method,
                event,
            } => {
                let message = format!("conductor does not handle {method}");
                tracing::warn!("{message}");
                match self.write(Outbound::Reject { rpc_id, message }).await {
                    Ok(()) => {}
                    Err(e) => tracing::error!("rejecting {method}: {e}"),
                }
                self.emit(event).await;
            }
        }
    }

// src/codex/server.rs:437-455 (rewritten)
    async fn on_response(&mut self, id: RequestId, outcome: Result<Value, String>) {
        let pending = match &id {
            RequestId::Integer(n) => self.pending.remove(n),
            RequestId::String(_) => None,
        };
        match pending {
            Some((method, reply)) => {
                match (method.as_str(), &outcome) {
                    (methods::INITIALIZE, Ok(_)) => match self.write(Outbound::Initialized).await {
                        Ok(()) => {}
                        Err(e) => tracing::error!("sending initialized: {e}"),
                    },
                    _ => {}
                }
                drop(reply.send(outcome.map_err(|message| Error::Rpc { method, message })));
            }
            None => tracing::warn!("response for unknown request {id}"),
        }
    }

// src/codex/server.rs:457-481 (rewritten)
    async fn on_command(&mut self, cmd: CommandMsg) {
        match cmd {
            CommandMsg::Request { call, reply } => {
                self.next_id = self.next_id.saturating_add(1);
                let id = self.next_id;
                let method = String::from(call.method());
                match self.write(Outbound::Request { id, call }).await {
                    Ok(()) => {
                        self.pending.insert(id, (method, reply));
                    }
                    Err(e) => drop(reply.send(Err(e))),
                }
            }
            CommandMsg::Respond {
                id,
                decision,
                reply,
            } => {
                drop(reply.send(self.respond(id, decision).await));
            }
            CommandMsg::Shutdown { reply } => {
                drop(reply.send(self.stop().await));
            }
        }
    }

// src/claude/session.rs:437-480 (rewritten)
    async fn on_command(&mut self, cmd: CommandMsg) {
        match cmd {
            CommandMsg::SendUser { text, reply } => {
                drop(reply.send(self.write(Outbound::User(text)).await));
            }
            CommandMsg::Control { call, reply } => {
                self.next_request = self.next_request.saturating_add(1);
                let request_id = format!("req_{}", self.next_request);
                match self
                    .write(Outbound::Control {
                        request_id: request_id.clone(),
                        call,
                    })
                    .await
                {
                    Ok(()) => {
                        self.pending.insert(request_id, (call, reply));
                    }
                    Err(e) => drop(reply.send(Err(e))),
                }
            }
            CommandMsg::Respond {
                id,
                decision,
                reply,
            } => {
                let result = match self.open_permissions.remove(&id) {
                    None => Err(Error::UnknownPermissionRequest(id)),
                    Some(input) => {
                        self.write(Outbound::Permission {
                            id,
                            decision,
                            input,
                        })
                        .await
                    }
                };
                drop(reply.send(result));
            }
            CommandMsg::Shutdown { reply } => {
                drop(reply.send(self.stop().await));
            }
        }
    }

// src/bin/s1.rs:530-548 (rewritten)
    let failed = Vec::from([
        approve,
        deny,
        interrupt,
        pending,
        resume,
        accept_edits,
        unknown_id,
    ])
    .into_iter()
    .filter_map(|verdict| match verdict {
        Verdict::Fail => Some(verdict),
        Verdict::Pass => None,
    })
    .count();
    match failed {
        0 => Ok(()),
        failed => Err(anyhow::Error::msg(format!("{failed} step(s) failed"))),
    }

// src/bin/s2.rs:591-601 (rewritten)
    let failed = Vec::from([approve, decline, interrupt, two_threads, resume, ids])
        .into_iter()
        .filter_map(|verdict| match verdict {
            Verdict::Fail => Some(verdict),
            Verdict::Pass => None,
        })
        .count();
    match failed {
        0 => Ok(()),
        failed => Err(anyhow::Error::msg(format!("{failed} step(s) failed"))),
    }

// tests/replay.rs:216-222 (rewritten)
    let results = recording
        .lines()
        .filter_map(|line| match frame_type(line).as_deref() {
            Some("result") => Some(line),
            _ => None,
        })
        .count();
