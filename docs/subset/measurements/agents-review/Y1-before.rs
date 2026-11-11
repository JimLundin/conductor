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
