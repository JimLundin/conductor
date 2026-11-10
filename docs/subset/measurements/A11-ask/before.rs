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
