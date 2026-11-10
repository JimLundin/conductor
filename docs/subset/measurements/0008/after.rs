impl Actor {
    async fn run(mut self, mut inbox: mpsc::Receiver<Msg>, host: Host) -> Result<Infallible, Stopped> {
        loop {
            let msg = inbox.recv().await.ok_or(Stopped {})?;
            self.on(msg, &host);
        }
    }

    fn on(&mut self, msg: Msg, host: &Host) {
        match msg {
            Msg::Welcome { reply } => drop(reply.send(self.welcome())),
            Msg::Subscribe { after_seq, reply } => drop(reply.send(self.subscription(after_seq))),
            Msg::Command { body, by, reply } => {
                let result = self.command(body, by, host);
                drop(reply.send(result));
            }
            Msg::Emit(kind) => self.emit(kind),
            Msg::OpenApproval { tool, detail, decided } => {
                let id = RequestId(self.next_request);
                self.next_request = self.next_request.saturating_add(1);
                self.approvals.insert(id, Approval::Open(decided));
                self.append(EventKind::ApprovalRequested { id, tool, detail });
            }
        }
    }

    fn emit(&mut self, kind: EventKind) {
        match &kind {
            EventKind::TurnFinished => self.turn = Turn::Idle,
            _ => {}
        }
        self.append(kind);
    }

    fn append(&mut self, kind: EventKind) {
        let event = Event { seq: self.head().next(), kind };
        self.log.push(event.clone());
        drop(self.live.send(event));
    }

    fn resolve(&mut self, approval: RequestId, decision: Decision, by: String) -> CommandResult {
        match self.approvals.remove(&approval) {
            None => CommandResult::Rejected(Rejection::UnknownApproval),
            Some(Approval::Resolved { by: first }) => {
                self.approvals.insert(approval, Approval::Resolved { by: first.clone() });
                CommandResult::Rejected(Rejection::AlreadyResolved { by: first })
            }
            Some(Approval::Open(open)) => {
                self.approvals.insert(approval, Approval::Resolved { by: by.clone() });
                self.append(EventKind::ApprovalResolved { id: approval, decision, by });
                drop(open.send(decision));
                CommandResult::Accepted
            }
        }
    }
}
