impl Actor {
    async fn on_response(&mut self, id: RequestId, outcome: Result<Value, String>) {
        let pending = match &id {
            RequestId::Integer(n) => self.pending.remove(n),
            RequestId::String(_) => None,
        };
        match pending {
            Some(Pending { method, reply }) => {
                if let (methods::INITIALIZE, Ok(_)) = (method.as_str(), &outcome) {
                    self.initialized().await;
                }
                let _ = reply.send(outcome.map_err(|message| Error::Rpc { method, message }));
            }
            None => tracing::warn!("response for unknown request {id}"),
        }
    }

    async fn initialized(&mut self) {
        if let Err(e) = self.write(Outbound::Initialized).await {
            tracing::error!("sending initialized: {e}");
        }
    }
}
