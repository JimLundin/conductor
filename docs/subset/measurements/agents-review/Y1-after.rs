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
