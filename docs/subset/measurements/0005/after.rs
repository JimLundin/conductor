pub async fn send_user(&self, text: String) -> Result<(), Error> {
    self.request(|reply| CommandMsg::SendUser { text, reply }).await?
}

pub async fn interrupt(&self) -> Result<(), Error> {
    self.control(ControlCall::Interrupt).await.map(drop)
}

async fn write(&mut self, msg: Outbound) -> Result<(), Error> {
    let running = self.running()?;
    let line = encode(msg)?;
    running.stdin.write_all(line.as_bytes()).await.map_err(Error::Io)?;
    running.stdin.flush().await.map_err(Error::Io)
}

fn main() -> Result<(), anyhow::Error> {
    tokio::runtime::Runtime::new()?.block_on(run())
}
