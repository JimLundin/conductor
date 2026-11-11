/// One thing the actor reacts to.
enum Input {
    Line(std::io::Result<Option<String>>),
    Command(CommandMsg),
    /// Every handle is gone.
    Closed,
}
    async fn run(
        mut self,
        mut commands: mpsc::Receiver<CommandMsg>,
    ) -> Result<Infallible, Stopped> {
        loop {
            let input = self.next_input(&mut commands).await;
            self.on_input(input).await?;
        }
    }

    async fn next_input(&mut self, commands: &mut mpsc::Receiver<CommandMsg>) -> Input {
        match &mut self.process {
            Process::Running(running) => tokio::select! {
                line = running.stdout.next_line() => Input::Line(line),
                cmd = commands.recv() => cmd.map_or(Input::Closed, Input::Command),
            },
            Process::Exited(_) => commands.recv().await.map_or(Input::Closed, Input::Command),
        }
    }

    async fn on_input(&mut self, input: Input) -> Result<(), Stopped> {
        match input {
            Input::Line(Ok(Some(line))) => {
                self.on_line(&line).await;
                Ok(())
            }
            Input::Line(Ok(None)) => {
                self.stop().await;
                Ok(())
            }
            Input::Line(Err(e)) => {
                tracing::error!("reading claude stdout: {e}");
                self.stop().await;
                Ok(())
            }
            Input::Command(cmd) => {
                self.on_command(cmd).await;
                Ok(())
            }
            Input::Closed => {
                self.stop().await;
                Err(Stopped {})
            }
        }
    }

