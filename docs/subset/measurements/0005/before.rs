/// Proof that an action finished and had nothing to report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Done {}

pub async fn send_user(&self, text: String) -> Result<Done, Error> {
    self.request(|reply| CommandMsg::SendUser { text, reply }).await?
}

pub async fn interrupt(&self) -> Result<Done, Error> {
    self.control(ControlCall::Interrupt).await.map(|_| Done {})
}

async fn write(&mut self, msg: Outbound) -> Result<Done, Error> {
    let running = self.running()?;
    let line = encode(msg)?;
    running.stdin.write_all(line.as_bytes()).await.map_err(Error::Io)?;
    running.stdin.flush().await.map_err(Error::Io)?;
    Ok(Done {})
}

fn main() -> Result<Done, anyhow::Error> {
    tokio::runtime::Runtime::new()?.block_on(run())
}
