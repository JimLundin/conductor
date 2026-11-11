    pub async fn send_user(&self, text: String) -> Result<(), Error> {
        self.ask(|reply| CommandMsg::SendUser { text, reply }).await?
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
        self.ask(|reply| CommandMsg::Respond {
            id,
            decision,
            reply,
        })
        .await?
    }

    /// Closes stdin, waits briefly for a clean exit, then kills.
    pub async fn shutdown(self) -> Result<ExitStatus, Error> {
        self.ask(|reply| CommandMsg::Shutdown { reply }).await
    }

    async fn control(&self, call: ControlCall) -> Result<Value, Error> {
        tokio::time::timeout(
            CONTROL_TIMEOUT,
            self.ask(|reply| CommandMsg::Control { call, reply }),
        )
        .await
        .map_err(|_| Error::Timeout(String::from(call.subtype())))??
    }

    async fn ask<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<T>) -> CommandMsg,
    ) -> Result<T, Error> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(build(reply))
            .await
            .map_err(|_| Error::Exited)?;
        answer.await.map_err(|_| Error::Exited)
    }
