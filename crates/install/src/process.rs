//! How a provider CLI child process is started and stopped.

use std::time::Duration;

use tokio::process::{Child, ChildStdin, Command};

use agents::Exit;

const EXIT_GRACE: Duration = Duration::from_secs(5);

/// Keeps a host started without a console, such as from Task Scheduler, from
/// opening a console window for each agent process.
#[cfg(not(unix))]
#[must_use]
pub fn no_window(mut cmd: Command) -> Command {
    use std::os::windows::process::CommandExt;
    /// `CREATE_NO_WINDOW` from the Win32 process creation flags.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.as_std_mut().creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(unix)]
#[must_use]
pub const fn no_window(cmd: Command) -> Command {
    cmd
}

/// Closes stdin, waits briefly for a clean exit, then kills.
pub async fn stop(mut child: Child, stdin: ChildStdin) -> Exit {
    drop(stdin);
    if let Ok(Ok(status)) = tokio::time::timeout(EXIT_GRACE, child.wait()).await
    {
        Exit::Status(status.to_string())
    } else {
        let _ = child.start_kill();
        match child.wait().await {
            Ok(status) => Exit::Status(status.to_string()),
            Err(e) => Exit::Unknown(e.to_string()),
        }
    }
}
