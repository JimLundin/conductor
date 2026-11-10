impl Actor {
    async fn run(mut self, mut commands: mpsc::Receiver<CommandMsg>) {
        loop {
            let Process::Running(running) = &mut self.process else {
                let Some(cmd) = commands.recv().await else {
                    return;
                };
                self.on_command(cmd).await;
                continue;
            };
            tokio::select! {
                line = running.stdout.next_line() => match line {
                    Ok(Some(line)) => self.on_line(&line).await,
                    Ok(None) => {
                        self.stop().await;
                    }
                    Err(e) => {
                        tracing::error!("reading claude stdout: {e}");
                        self.stop().await;
                    }
                },
                cmd = commands.recv() => match cmd {
                    Some(cmd) => self.on_command(cmd).await,
                    None => {
                        self.stop().await;
                        return;
                    }
                },
            }
        }
    }
}
