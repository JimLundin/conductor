async fn write(&mut self, msg: Outbound) -> Result<(), Error> {
    let running = self.running()?;
    let line = encode(msg).map_err(Error::Encode)?;
    running.stdin.write_all(line.as_bytes()).await.map_err(Error::Io)?;
    running.stdin.flush().await.map_err(Error::Io)?;
    Ok(())
}
fn stdout_log(opts: &ClaudeOptions) -> Result<Option<std::fs::File>, Error> {
    match &opts.stdout_log {
        Some(path) => Ok(Some(std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(Error::Io)?)),
        None => Ok(None),
    }
}
