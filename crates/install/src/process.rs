//! How a provider CLI child process is started and stopped, and how its
//! sign-in runs on pipes.

use std::io;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};

use agents::{Exit, Invocation, Var};

use crate::Home;
mod owned;
pub(crate) use owned::Child;
pub use owned::Processes;

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

/// Closes stdin, waits briefly for a clean exit, then kills.
pub(crate) async fn stop(child: Child, stdin: ChildStdin) -> Exit {
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

async fn killed(child: Child) -> std::io::Result<ExitStatus> {
    let _ = child.start_kill();
    tokio::time::timeout(EXIT_GRACE, child.wait())
        .await
        .map_err(io::Error::other)?
}

/// A CLI's sign-in, running with its output on pipes. Dropping it kills
/// the CLI.
#[derive(Debug)]
pub struct SignInRun {
    child: Child,
    out: ChildStdout,
    err: ChildStderr,
}

/// Starts `program` as `run` says, in `home`, for a sign-in, and returns
/// it with its stdin.
///
/// # Errors
///
/// Why it could not start.
pub fn sign_in(
    program: &Path,
    home: &Home,
    run: Invocation,
) -> io::Result<(SignInRun, ChildStdin)> {
    let (child, stdin, out, err) = piped(command(program, home, run))?;
    Ok((SignInRun { child, out, err }, stdin))
}

pub(crate) fn piped(
    command: Command,
) -> io::Result<(Child, ChildStdin, ChildStdout, ChildStderr)> {
    piped_owned(command, &Processes::default())
}

pub(crate) fn piped_owned(
    command: Command,
    processes: &Processes,
) -> io::Result<(Child, ChildStdin, ChildStdout, ChildStderr)> {
    let mut cmd = command;
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    owned::spawn(cmd, processes)
}

fn unpiped(stream: &str) -> io::Error {
    io::Error::other(format!("the child's {stream} is not a pipe"))
}

impl SignInRun {
    /// Hands each line the CLI prints, on either stream, to `printed`, and
    /// returns how it exited once both streams have closed.
    ///
    /// # Errors
    ///
    /// Why waiting for it failed.
    pub async fn finish(
        self,
        printed: impl Fn(String),
    ) -> io::Result<ExitStatus> {
        tokio::join!(lines(self.out, &printed), lines(self.err, &printed));
        self.child.wait().await
    }
}

/// Each line of `stream`, invalid UTF-8 replaced, until it closes or fails.
async fn lines(stream: impl AsyncRead + Unpin, printed: &impl Fn(String)) {
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line).await.unwrap_or(0) > 0 {
        printed(
            String::from_utf8_lossy(&line)
                .trim_end_matches(['\r', '\n'])
                .to_owned(),
        );
        line.clear();
    }
}

/// Writes `line` to a sign-in's stdin, ended with a newline.
///
/// # Errors
///
/// Why the write failed, for instance because the CLI has exited.
pub async fn write_line(stdin: &mut ChildStdin, line: &str) -> io::Result<()> {
    stdin.write_all(line.as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await
}
