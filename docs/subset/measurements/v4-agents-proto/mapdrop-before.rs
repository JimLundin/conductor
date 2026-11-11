impl ClaudeSession {
    pub async fn interrupt(&self) -> Result<(), Error> {
        self.control(ControlCall::Interrupt).await.map(drop)
    }

    pub async fn set_permission_mode(&self, mode: PermissionMode) -> Result<(), Error> {
        self.control(ControlCall::SetPermissionMode(mode))
            .await
            .map(drop)
    }
}

impl CodexThread {
    pub async fn interrupt(&self, turn: TurnId) -> Result<(), Error> {
        let call = Call::TurnInterrupt {
            thread: self.id.clone(),
            turn,
        };
        request(&self.server.commands, call).await.map(drop)
    }
}

pub(crate) fn check_version(initialize: Value) -> Result<(), UnsupportedVersion> {
    let found = serde_json::from_value::<InitializeResponse>(initialize)
        .map_err(|_| UnsupportedVersion::Unreported)?
        .claude_code_version;
    claude_codes::check_cli_version(&found)
        .map(drop)
        .map_err(|e| UnsupportedVersion::Outside {
            found: e.found,
            supported: claude_codes::version::supported_cli_versions(),
        })
}
