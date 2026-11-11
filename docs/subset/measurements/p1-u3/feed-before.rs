impl Conductor {
    async fn feed(
        &self,
        transport: &Transport,
        mut send: SendHalf,
        subscribe: Subscribe,
    ) -> Result<(), StreamError> {
        let welcome = self.log.welcome().await?;
        match Some(welcome).filter(|welcome| welcome.epoch == subscribe.epoch) {
            // A cursor from another epoch is refused by closing the stream.
            None => Ok(send.finish()?),
            Some(_) => {
                let subscription = self.log.subscribe(subscribe.after_seq).await?;
                tracing::info!(
                    transport = transport.name(),
                    after_seq = subscribe.after_seq.0,
                    replayed = subscription.replay.len(),
                    "subscribed"
                );
                let feed = Feed {
                    send,
                    replay: subscription.replay.into_iter(),
                    live: subscription.live,
                    last: subscribe.after_seq,
                };
                let result = feed.run(transport).await;
                tracing::info!(transport = transport.name(), ?result, "subscription ended");
                result
            }
        }
    }
}
