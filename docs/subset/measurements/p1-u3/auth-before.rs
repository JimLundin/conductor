impl Conductor {
    /// The first stream must be a `Hello` with the right token. Returns the
    /// client's name, or `None` if it was refused.
    async fn authenticate(&self, conn: &Connection) -> Result<Option<ClientName>, StreamError> {
        let (send, recv) = conn.accept_bi().await?;
        let (mut send, mut recv) = (SendHalf::Iroh(send), RecvHalf::Iroh(recv));
        let admitted = match recv.read::<Open>().await? {
            Some(Open::Hello(hello)) => Some(hello)
                .filter(|hello| hello.token == self.token)
                .ok_or("wrong token"),
            Some(Open::Subscribe(_) | Open::Command(_) | Open::Rtc(_)) | None => {
                Err("the first stream must be a hello")
            }
        };
        match admitted {
            Ok(hello) => {
                send.write(&HelloReply::Welcome(self.log.welcome().await?))
                    .await?;
                send.finish()?;
                Ok(Some(hello.client))
            }
            Err(reason) => {
                send.write(&HelloReply::Refused {
                    reason: String::from(reason),
                })
                .await?;
                send.finish()?;
                // Let the refusal reach the client before the close does.
                send.stopped().await;
                Ok(None)
            }
        }
    }
}
