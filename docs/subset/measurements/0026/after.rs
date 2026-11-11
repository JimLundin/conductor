// crates/agents/src/claude/session.rs:171-224 with one ask<T>
    pub async fn send_user(&self, text: String) -> Result<(), Error> {
        self.ask(|reply| CommandMsg::SendUser { text, reply }).await?
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
        self.ask(|reply| CommandMsg::Respond {
            id,
            decision,
            reply,
        })
        .await?
    }

    /// Closes stdin, waits briefly for a clean exit, then kills.
    pub async fn shutdown(self) -> Result<ExitStatus, Error> {
        self.ask(|reply| CommandMsg::Shutdown { reply }).await
    }

    async fn control(&self, call: ControlCall) -> Result<Value, Error> {
        tokio::time::timeout(
            CONTROL_TIMEOUT,
            self.ask(|reply| CommandMsg::Control { call, reply }),
        )
        .await
        .map_err(|_| Error::Timeout(String::from(call.subtype())))??
    }

    async fn ask<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<T>) -> CommandMsg,
    ) -> Result<T, Error> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(build(reply))
            .await
            .map_err(|_| Error::Exited)?;
        answer.await.map_err(|_| Error::Exited)
    }

// crates/agents/src/codex/server.rs:167-187, 227-243 with one ask<T>
    /// Answers an open approval or user-input request exactly once.
    pub async fn respond(&self, id: ApprovalId, decision: Decision) -> Result<(), Error> {
        ask(&self.commands, |reply| CommandMsg::Respond {
            id,
            decision,
            reply,
        })
        .await?
    }

    /// Closes stdin, waits briefly for a clean exit, then kills.
    pub async fn shutdown(&self) -> Result<ExitStatus, Error> {
        ask(&self.commands, |reply| CommandMsg::Shutdown { reply }).await
    }
/// Sends `call` to the actor and waits for Codex's answer.
async fn request(commands: &mpsc::Sender<CommandMsg>, call: Call) -> Result<Value, Error> {
    let method = String::from(call.method());
    tokio::time::timeout(
        REQUEST_TIMEOUT,
        ask(commands, |reply| CommandMsg::Request { call, reply }),
    )
    .await
    .map_err(|_| Error::Timeout(method))??
}

async fn ask<T>(
    commands: &mpsc::Sender<CommandMsg>,
    build: impl FnOnce(oneshot::Sender<T>) -> CommandMsg,
) -> Result<T, Error> {
    let (reply, answer) = oneshot::channel();
    commands
        .send(build(reply))
        .await
        .map_err(|_| Error::Exited)?;
    answer.await.map_err(|_| Error::Exited)
}

// crates/daemon/src/host.rs, from docs/subset/measurements/A11-ask/after.rs
impl Host {
    pub async fn welcome(&self) -> Result<Welcome, Stopped> {
        self.ask(|reply| Msg::Welcome { reply }).await
    }

    pub async fn subscribe(&self, after_seq: HostSeq) -> Result<Subscription, Stopped> {
        self.ask(|reply| Msg::Subscribe { after_seq, reply }).await
    }

    pub async fn command(&self, body: CommandBody, by: String) -> Result<CommandResult, Stopped> {
        self.ask(|reply| Msg::Command { body, by, reply }).await
    }

    async fn ask<T>(&self, msg: impl FnOnce(oneshot::Sender<T>) -> Msg) -> Result<T, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.messages.send(msg(reply)).await.map_err(|_| Stopped {})?;
        answer.await.map_err(|_| Stopped {})
    }
}
