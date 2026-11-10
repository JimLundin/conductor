impl Actor {
    async fn run(self, inbox: mpsc::Receiver<Msg>, host: Host) {
        ReceiverStream::new(inbox)
            .fold(self, |actor, msg| ready(actor.on(msg, &host)))
            .await;
    }

    fn on(self, msg: Msg, host: &Host) -> Actor {
        match msg {
            Msg::Welcome { reply } => {
                drop(reply.send(self.welcome()));
                self
            }
            Msg::Subscribe { after_seq, reply } => {
                drop(reply.send(self.subscription(after_seq)));
                self
            }
            Msg::Command { body, by, reply } => {
                let (actor, result) = self.command(body, by, host);
                drop(reply.send(result));
                actor
            }
            Msg::Emit(kind) => self.emit(kind),
            Msg::OpenApproval { tool, detail, decided } => {
                let id = RequestId(self.next_request);
                let approvals = with(self.approvals, id, Approval::Open(decided));
                Actor { approvals, next_request: self.next_request.saturating_add(1), ..self }
                    .append(EventKind::ApprovalRequested { id, tool, detail })
            }
        }
    }

    fn emit(self, kind: EventKind) -> Actor {
        let turn = match &kind {
            EventKind::TurnFinished => Turn::Idle,
            _ => self.turn,
        };
        Actor { turn, ..self }.append(kind)
    }

    fn append(self, kind: EventKind) -> Actor {
        let event = Event { seq: self.head().next(), kind };
        drop(self.live.send(event.clone()));
        let log = self.log.into_iter().chain(std::iter::once(event)).collect();
        Actor { log, ..self }
    }

    fn resolve(self, approval: RequestId, decision: Decision, by: String) -> (Actor, CommandResult) {
        let (found, rest) = split_off(self.approvals, approval);
        match found.into_values().next() {
            None => (Actor { approvals: rest, ..self }, CommandResult::Rejected(Rejection::UnknownApproval)),
            Some(Approval::Resolved { by: first }) => {
                let approvals = with(rest, approval, Approval::Resolved { by: first.clone() });
                (Actor { approvals, ..self }, CommandResult::Rejected(Rejection::AlreadyResolved { by: first }))
            }
            Some(Approval::Open(open)) => {
                let approvals = with(rest, approval, Approval::Resolved { by: by.clone() });
                let actor = Actor { approvals, ..self }
                    .append(EventKind::ApprovalResolved { id: approval, decision, by });
                drop(open.send(decision));
                (actor, CommandResult::Accepted)
            }
        }
    }
}

fn with(approvals: HashMap<RequestId, Approval>, id: RequestId, approval: Approval) -> HashMap<RequestId, Approval> {
    approvals.into_iter().chain(std::iter::once((id, approval))).collect()
}

fn split_off(approvals: HashMap<RequestId, Approval>, id: RequestId) -> (HashMap<RequestId, Approval>, HashMap<RequestId, Approval>) {
    approvals.into_iter().partition(|(key, _)| key.eq(&id))
}
