impl Conductor {
    async fn feed(
        &self,
        transport: &Transport,
        mut send: SendHalf,
        subscribe: Subscribe,
    ) -> Result<(), StreamError> {
        match self.log.welcome().await? {
            Welcome { epoch, .. } if epoch == subscribe.epoch => {
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
            // A cursor from another epoch is refused by closing the stream.
            Welcome { .. } => Ok(send.finish()?),
        }
    }
}
