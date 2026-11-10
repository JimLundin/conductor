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
