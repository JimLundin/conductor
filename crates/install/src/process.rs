//! How a provider CLI child process is started and stopped.

use std::path::Path;
use std::process::ExitStatus;
use std::time::Duration;

use tokio::process::{Child, ChildStdin, Command};

use agents::{Exit, Invocation, Var};

use crate::Home;

const EXIT_GRACE: Duration = Duration::from_secs(5);

/// `program` run as `run` says, in `home`.
#[must_use]
pub fn command(program: &Path, home: &Home, run: Invocation) -> Command {
    let mut cmd = Command::new(program);
    for var in run.env {
        set(&mut cmd, var, home);
    }
    cmd.args(run.args);
    cmd
}

fn set(cmd: &mut Command, var: Var, home: &Home) {
    match (var, home) {
        (Var::Set { name, value }, _) => cmd.env(name, value),
        (Var::Remove(name), _) => cmd.env_remove(name),
        (Var::Home(name), Home::Private(dir)) => cmd.env(name, dir),
        (Var::Home(_), Home::User) => cmd,
    };
}

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
    let graceful = tokio::time::timeout(EXIT_GRACE, child.wait())
        .await
        .ok()
        .and_then(Result::ok);
    waited(child, graceful).await.map_or_else(
        |error| Exit::Unknown(error.to_string()),
        |status| Exit::Status(status.to_string()),
    )
}

async fn waited(
    child: Child,
    graceful: Option<ExitStatus>,
) -> std::io::Result<ExitStatus> {
    match graceful {
        Some(status) => Ok(status),
        None => killed(child).await,
    }
}

async fn killed(mut child: Child) -> std::io::Result<ExitStatus> {
    let _ = child.start_kill();
    child.wait().await
}
