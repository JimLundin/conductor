impl Link {
    async fn adopt(&mut self, welcome: &Welcome) -> Result<(), Stopped> {
        match self.cursor.filter(|cursor| cursor.epoch == welcome.epoch) {
            Some(_) => Ok(()),
            None => {
                self.cursor = Some(Cursor {
                    epoch: welcome.epoch,
                    seq: HostSeq(0),
                });
                self.outbox.emit(Update::Reset(welcome.epoch)).await
            }
        }
    }
}
impl Session {
    async fn went_live(&mut self) -> anyhow::Result<Flow> {
        match self.prompted_after {
            Some(_) => Ok(Flow::Next),
            None => {
                self.prompted_after = Some(self.last_seq);
                let text = self.prompt.clone();
                let result = self.link.send(CommandBody::Prompt { text }).await?;
                self.log(format!("prompt {:?} -> {result:?}", self.prompt))?;
                Ok(Flow::Next)
            }
        }
    }
}
