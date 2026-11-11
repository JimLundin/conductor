//! How a provider CLI child process is stopped.

use std::time::Duration;

use tokio::process::{Child, ChildStdin};

use crate::Exit;

const EXIT_GRACE: Duration = Duration::from_secs(5);

/// Closes stdin, waits briefly for a clean exit, then kills.
pub(crate) async fn stop(mut child: Child, stdin: ChildStdin) -> Exit {
    drop(stdin);
    match tokio::time::timeout(EXIT_GRACE, child.wait()).await {
        Ok(Ok(status)) => Exit::Status(status),
        Ok(Err(_)) | Err(_) => {
            let _ = child.start_kill();
            match child.wait().await {
                Ok(status) => Exit::Status(status),
                Err(e) => Exit::Unknown(e.to_string()),
            }
        }
    }
}
