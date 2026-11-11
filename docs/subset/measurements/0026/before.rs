// crates/agents/src/claude/session.rs:171-224
    pub async fn send_user(&self, text: String) -> Result<(), Error> {
        let (reply, answer) = oneshot::channel();
        self.send(CommandMsg::SendUser { text, reply }).await?;
        answer.await.map_err(|_| Error::Exited)?
    }

    /// Resolves once the CLI acknowledges; the turn's `result` follows as an event.
    pub async fn interrupt(&self) -> Result<(), Error> {
        self.control(ControlCall::Interrupt).await.map(drop)
    }

    pub async fn set_permission_mode(&self, mode: PermissionMode) -> Result<(), Error> {
        self.control(ControlCall::SetPermissionMode(mode))
            .await
            .map(drop)
    }

    pub async fn respond(
        &self,
        id: PermissionRequestId,
        decision: PermissionDecision,
    ) -> Result<(), Error> {
        let (reply, answer) = oneshot::channel();
        self.send(CommandMsg::Respond {
            id,
            decision,
            reply,
        })
        .await?;
        answer.await.map_err(|_| Error::Exited)?
    }

    /// Closes stdin, waits briefly for a clean exit, then kills.
    pub async fn shutdown(self) -> Result<ExitStatus, Error> {
        let (reply, answer) = oneshot::channel();
        self.send(CommandMsg::Shutdown { reply }).await?;
        answer.await.map_err(|_| Error::Exited)
    }

    async fn control(&self, call: ControlCall) -> Result<Value, Error> {
        tokio::time::timeout(CONTROL_TIMEOUT, self.ask_control(call))
            .await
            .map_err(|_| Error::Timeout(String::from(call.subtype())))?
    }

    async fn ask_control(&self, call: ControlCall) -> Result<Value, Error> {
        let (reply, answer) = oneshot::channel();
        self.send(CommandMsg::Control { call, reply }).await?;
        answer.await.map_err(|_| Error::Exited)?
    }

    async fn send(&self, msg: CommandMsg) -> Result<(), Error> {
        self.commands.send(msg).await.map_err(|_| Error::Exited)
    }

// crates/agents/src/codex/server.rs:167-187, 227-243
    /// Answers an open approval or user-input request exactly once.
    pub async fn respond(&self, id: ApprovalId, decision: Decision) -> Result<(), Error> {
        let (reply, answer) = oneshot::channel();
        send(
            &self.commands,
            CommandMsg::Respond {
                id,
                decision,
                reply,
            },
        )
        .await?;
        answer.await.map_err(|_| Error::Exited)?
    }

    /// Closes stdin, waits briefly for a clean exit, then kills.
    pub async fn shutdown(&self) -> Result<ExitStatus, Error> {
        let (reply, answer) = oneshot::channel();
        send(&self.commands, CommandMsg::Shutdown { reply }).await?;
        answer.await.map_err(|_| Error::Exited)
    }
/// Sends `call` to the actor and waits for Codex's answer.
async fn request(commands: &mpsc::Sender<CommandMsg>, call: Call) -> Result<Value, Error> {
    let method = String::from(call.method());
    tokio::time::timeout(REQUEST_TIMEOUT, ask(commands, call))
        .await
        .map_err(|_| Error::Timeout(method))?
}

async fn ask(commands: &mpsc::Sender<CommandMsg>, call: Call) -> Result<Value, Error> {
    let (reply, answer) = oneshot::channel();
    send(commands, CommandMsg::Request { call, reply }).await?;
    answer.await.map_err(|_| Error::Exited)?
}

async fn send(commands: &mpsc::Sender<CommandMsg>, msg: CommandMsg) -> Result<(), Error> {
    commands.send(msg).await.map_err(|_| Error::Exited)
}

// crates/daemon/src/host.rs, from docs/subset/measurements/A11-ask/before.rs
impl Host {
    pub async fn welcome(&self) -> Result<Welcome, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.messages.send(Msg::Welcome { reply }).await.map_err(|_| Stopped {})?;
        answer.await.map_err(|_| Stopped {})
    }

    pub async fn subscribe(&self, after_seq: HostSeq) -> Result<Subscription, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.messages.send(Msg::Subscribe { after_seq, reply }).await.map_err(|_| Stopped {})?;
        answer.await.map_err(|_| Stopped {})
    }

    pub async fn command(&self, body: CommandBody, by: String) -> Result<CommandResult, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.messages.send(Msg::Command { body, by, reply }).await.map_err(|_| Stopped {})?;
        answer.await.map_err(|_| Stopped {})
    }
}
