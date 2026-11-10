#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not build the claude command: {0}")]
    Command(String),
    #[error("claude i/o: {0}")]
    Io(std::io::Error),
    #[error("could not encode a message for claude: {0}")]
    Encode(serde_json::Error),
    #[error("the claude process has exited")]
    Exited,
    #[error("claude rejected {subtype}: {message}")]
    Control { subtype: String, message: String },
    #[error("claude did not answer {0} within {CONTROL_TIMEOUT:?}")]
    Timeout(String),
    #[error("claude {found} is not supported (supported: {supported}); set allow_untested_version to run it anyway")]
    UnsupportedVersion { found: String, supported: String },
    #[error("no open permission request with id {}", .0.text())]
    UnknownPermissionRequest(PermissionRequestId),
}
