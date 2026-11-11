/// One thing the actor reacts to.
enum Input {
    Line(std::io::Result<Option<String>>),
    Command(CommandMsg),
}
    async fn run(
        mut self,
        mut commands: mpsc::Receiver<CommandMsg>,
    ) -> Result<Infallible, Stopped> {
        loop {
            let input = self.next_input(&mut commands).await?;
            self.on_input(input).await;
        }
    }

    /// The next input, or `Stopped` once every handle is gone.
    async fn next_input(
        &mut self,
        commands: &mut mpsc::Receiver<CommandMsg>,
    ) -> Result<Input, Stopped> {
        let input = match &mut self.process {
            Process::Running(running) => tokio::select! {
                line = running.stdout.next_line() => Some(Input::Line(line)),
                cmd = commands.recv() => cmd.map(Input::Command),
            },
            Process::Exited(_) => commands.recv().await.map(Input::Command),
        };
        match input {
            Some(input) => Ok(input),
            None => {
                self.stop().await;
                Err(Stopped {})
            }
        }
    }

    async fn on_input(&mut self, input: Input) {
        match input {
            Input::Line(Ok(Some(line))) => self.on_line(&line).await,
            Input::Line(Ok(None)) => drop(self.stop().await),
            Input::Line(Err(e)) => {
                tracing::error!("reading claude stdout: {e}");
                drop(self.stop().await);
            }
            Input::Command(cmd) => self.on_command(cmd).await,
        }
    }

