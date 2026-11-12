//! How a provider CLI child process is started and stopped.

use std::time::Duration;

use tokio::process::{Child, ChildStdin, Command};

use crate::Exit;

const EXIT_GRACE: Duration = Duration::from_secs(5);

/// Keeps a host started without a console, such as from Task Scheduler, from
/// opening a console window for each agent process.
#[cfg(not(unix))]
pub(crate) fn no_window(mut cmd: Command) -> Command {
    use std::os::windows::process::CommandExt;
    /// `CREATE_NO_WINDOW` from the Win32 process creation flags.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.as_std_mut().creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(unix)]
pub(crate) fn no_window(cmd: Command) -> Command {
    cmd
}

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
