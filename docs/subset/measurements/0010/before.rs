#[derive(Debug)]
pub enum Error {
    Command(String),
    Io(std::io::Error),
    Encode(serde_json::Error),
    Exited,
    Control { subtype: String, message: String },
    Timeout(String),
    UnsupportedVersion { found: String, supported: String },
    UnknownPermissionRequest(PermissionRequestId),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Command(e) => write!(f, "could not build the claude command: {e}"),
            Error::Io(e) => write!(f, "claude i/o: {e}"),
            Error::Encode(e) => write!(f, "could not encode a message for claude: {e}"),
            Error::Exited => write!(f, "the claude process has exited"),
            Error::Control { subtype, message } => write!(f, "claude rejected {subtype}: {message}"),
            Error::Timeout(call) => write!(f, "claude did not answer {call} within {CONTROL_TIMEOUT:?}"),
            Error::UnsupportedVersion { found, supported } => write!(
                f,
                "claude {found} is not supported (supported: {supported}); set allow_untested_version to run it anyway"
            ),
            Error::UnknownPermissionRequest(id) => {
                write!(f, "no open permission request with id {}", id.text())
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            Error::Encode(e) => Some(e),
            _ => None,
        }
    }
}
