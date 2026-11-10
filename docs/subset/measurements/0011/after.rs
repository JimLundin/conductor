async fn write(&mut self, msg: Outbound) -> Result<(), Error> {
    let running = self.running()?;
    let line = encode(msg)?;
    running.stdin.write_all(line.as_bytes()).await?;
    running.stdin.flush().await?;
    Ok(())
}
fn stdout_log(opts: &ClaudeOptions) -> Result<Option<std::fs::File>, Error> {
    match &opts.stdout_log {
        Some(path) => Ok(Some(std::fs::OpenOptions::new().create(true).append(true).open(path)?)),
        None => Ok(None),
    }
}
