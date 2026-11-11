    pub async fn send_user(&self, text: String) -> Result<(), Error> {
        let (reply, answer) = oneshot::channel();
        self.send(CommandMsg::SendUser { text, reply }).await?;
        answer.await.map_err(|_| Error::Exited)?
    }

    /// Resolves once the CLI acknowledges; the turn's `result` follows as an event.
    pub async fn interrupt(&self) -> Result<(), Error> {
        self.control(ControlCall::Interrupt).await.map(drop)
    }

    pub async fn set_permission_mode(&self, mode: PermissionMode) -> Result<(), Error> {
        self.control(ControlCall::SetPermissionMode(mode))
            .await
            .map(drop)
    }

    pub async fn respond(
        &self,
        id: PermissionRequestId,
        decision: PermissionDecision,
    ) -> Result<(), Error> {
        let (reply, answer) = oneshot::channel();
        self.send(CommandMsg::Respond {
            id,
            decision,
            reply,
        })
        .await?;
        answer.await.map_err(|_| Error::Exited)?
    }

    /// Closes stdin, waits briefly for a clean exit, then kills.
    pub async fn shutdown(self) -> Result<ExitStatus, Error> {
        let (reply, answer) = oneshot::channel();
        self.send(CommandMsg::Shutdown { reply }).await?;
        answer.await.map_err(|_| Error::Exited)
    }

    async fn control(&self, call: ControlCall) -> Result<Value, Error> {
        tokio::time::timeout(CONTROL_TIMEOUT, self.ask_control(call))
            .await
            .map_err(|_| Error::Timeout(String::from(call.subtype())))?
    }

    async fn ask_control(&self, call: ControlCall) -> Result<Value, Error> {
        let (reply, answer) = oneshot::channel();
        self.send(CommandMsg::Control { call, reply }).await?;
        answer.await.map_err(|_| Error::Exited)?
    }

    async fn send(&self, msg: CommandMsg) -> Result<(), Error> {
        self.commands.send(msg).await.map_err(|_| Error::Exited)
    }
