/// The command channel closed: nobody can talk to the actor any more.
struct Stopped {}

impl Actor {
    async fn run(mut self, mut commands: mpsc::Receiver<CommandMsg>) -> Result<Infallible, Stopped> {
        loop {
            match &mut self.process {
                Process::Running(running) => tokio::select! {
                    line = running.stdout.next_line() => self.on_stdout(line).await,
                    cmd = commands.recv() => self.on_command(cmd).await?,
                },
                Process::Exited(_) => self.on_command(commands.recv().await).await?,
            }
        }
    }

    async fn on_stdout(&mut self, line: std::io::Result<Option<String>>) {
        match line {
            Ok(Some(line)) => self.on_line(&line).await,
            Ok(None) => {
                self.stop().await;
            }
            Err(e) => {
                tracing::error!("reading claude stdout: {e}");
                self.stop().await;
            }
        }
    }

    async fn on_command(&mut self, cmd: Option<CommandMsg>) -> Result<(), Stopped> {
        match cmd {
            Some(cmd) => {
                self.on_message(cmd).await;
                Ok(())
            }
            None => {
                self.stop().await;
                Err(Stopped {})
            }
        }
    }
}
