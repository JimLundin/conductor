//! Executes a driver's ordered effects against one child process.

use std::fs::{File, OpenOptions};
use std::future::pending;
use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use agents::driver::{Command as DriverCommand, Effect, Failure, Input};
use agents::{Exit, Note};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader, Lines};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{Semaphore, mpsc, oneshot, watch};

use crate::process;
use crate::runner::{self, End};

pub type Reply<E> = oneshot::Sender<Result<Value, E>>;
type Waiter = oneshot::Sender<Exit>;
type Event<C, T, A> = Input<C, T, Waiter, A, io::Error>;
type Action<V, E, T, A, P> =
    Effect<V, Reply<E>, Waiter, T, A, Failure<io::Error, P>>;

#[derive(Clone, Copy)]
pub enum StderrPolicy {
    /// A handshake can wait for the last line, including a blank line.
    Last,
    /// Wait for the last nonempty line on stop and feed it into the driver.
    LastNonempty(Duration),
}

impl StderrPolicy {
    fn keeps(self, line: &str) -> bool {
        matches!(self, Self::Last) || !line.trim().is_empty()
    }
}

pub struct Stderr(watch::Receiver<Option<String>>);

impl Stderr {
    pub async fn last(&mut self, timeout: Duration) -> Option<String> {
        tokio::time::timeout(timeout, self.0.changed())
            .await
            .ok()?
            .ok()?;
        self.0.borrow_and_update().clone()
    }
}

pub struct Options {
    pub stdout_log: Option<File>,
    pub stderr: StderrPolicy,
    pub log_stderr: fn(&str),
    pub log_note: fn(&Note),
    pub commands: usize,
    pub events: usize,
}

pub struct Started<C, V> {
    pub handle: Handle<C>,
    pub events: mpsc::Receiver<V>,
    pub stderr: Stderr,
    pub pid: Option<u32>,
}

pub struct Handle<C> {
    mailbox: mpsc::Sender<DriverCommand<C, Waiter>>,
    calls: Arc<Semaphore>,
    closing: watch::Sender<()>,
}

impl<C> Clone for Handle<C> {
    fn clone(&self) -> Self {
        Self {
            mailbox: self.mailbox.clone(),
            calls: self.calls.clone(),
            closing: self.closing.clone(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("the process runner is unavailable")]
pub struct Unavailable;

impl<C> Handle<C> {
    /// # Errors
    /// Returns a receive error if the runner disappears before answering.
    pub async fn ask<T>(
        &self,
        call: impl FnOnce(oneshot::Sender<T>) -> C,
    ) -> Result<T, Unavailable> {
        self.enqueue(call)
            .await
            .ok_or(Unavailable)?
            .await
            .map_err(|_| Unavailable)
    }

    pub async fn enqueue<T>(
        &self,
        call: impl FnOnce(oneshot::Sender<T>) -> C,
    ) -> Option<oneshot::Receiver<T>> {
        let (reply, answer) = oneshot::channel();
        match self.calls.acquire().await {
            Ok(slot) => {
                self.mailbox
                    .send(DriverCommand::Call(call(reply)))
                    .await
                    .ok()?;
                slot.forget();
                Some(answer)
            }
            Err(_) => None,
        }
    }

    /// # Errors
    /// Returns a receive error if the runner disappears before answering.
    pub async fn shutdown(&self) -> Result<Exit, oneshot::error::RecvError> {
        let (waiter, exit) = oneshot::channel();
        if self
            .mailbox
            .send(DriverCommand::Shutdown(waiter))
            .await
            .is_ok()
        {
            self.closing.send_replace(());
        }
        exit.await
    }
}

/// # Errors
/// Returns an error if the log cannot be opened for appending.
pub fn stdout_log(path: Option<&Path>) -> io::Result<Option<File>> {
    path.map(|file| OpenOptions::new().create(true).append(true).open(file))
        .transpose()
}

/// # Errors
/// Returns an error if the child or one of its stdio pipes cannot be started.
pub fn start<C, V, E, T, A, P>(
    command: Command,
    options: Options,
    step: impl FnMut(Event<C, T, A>) -> Vec<Action<V, E, T, A, P>> + Send + 'static,
) -> io::Result<Started<C, V>>
where
    C: Send + 'static,
    V: Send + 'static,
    E: From<Failure<io::Error, P>> + Send + 'static,
    T: Send + 'static,
    A: Send + 'static,
    P: Send + 'static,
{
    let (child, stdin, stdout, stderr) = process::piped(command)?;
    let pid = child.id();
    let (last_tx, last_rx) = watch::channel(None);
    tokio::spawn(log_stderr(
        stderr,
        last_tx,
        options.stderr,
        options.log_stderr,
    ));

    let (events_tx, events) = mpsc::channel(options.events);
    // Calls use at most all but one slot, leaving room for a shutdown.
    let (mailbox_tx, mailbox) = mpsc::channel(options.commands + 1);
    let calls = Arc::new(Semaphore::new(options.commands));
    let (closing_tx, closing) = watch::channel(());
    let runner = Runner {
        process: Some(Running {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            stdout_log: options.stdout_log,
            stderr: match options.stderr {
                StderrPolicy::Last => None,
                StderrPolicy::LastNonempty(wait) => {
                    Some((Stderr(last_rx.clone()), wait))
                }
            },
        }),
        log_note: options.log_note,
        events: events_tx,
        mailbox,
        calls: calls.clone(),
        closing,
        interrupted: false,
    };
    tokio::spawn(runner::run(runner, step));
    Ok(Started {
        handle: Handle {
            mailbox: mailbox_tx,
            calls,
            closing: closing_tx,
        },
        events,
        stderr: Stderr(last_rx),
        pid,
    })
}

async fn log_stderr(
    stderr: ChildStderr,
    finished: watch::Sender<Option<String>>,
    policy: StderrPolicy,
    log: fn(&str),
) {
    let mut last = None;
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        log(&line);
        if policy.keeps(&line) {
            last = Some(line);
        }
    }
    let _ = finished.send(last);
}

struct Running {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    stdout_log: Option<File>,
    stderr: Option<(Stderr, Duration)>,
}

impl Running {
    async fn read(&mut self) -> Result<Option<String>, String> {
        let read = self.stdout.next_line().await;
        if let (Some(log), Ok(Some(line))) = (self.stdout_log.as_mut(), &read)
            && let Err(error) = writeln!(log, "{line}")
        {
            tracing::warn!("writing stdout log: {error}");
        }
        read.map_err(|error| error.to_string())
    }

    async fn stop(self) -> (Exit, Option<String>) {
        let exit = process::stop(self.child, self.stdin).await;
        let stderr = match self.stderr {
            Some((mut stderr, wait)) => stderr.last(wait).await,
            None => None,
        };
        (exit, stderr)
    }
}

async fn read(process: Option<&mut Running>) -> Result<Option<String>, String> {
    match process {
        Some(running) => running.read().await,
        None => pending().await,
    }
}

struct Runner<C, V> {
    process: Option<Running>,
    events: mpsc::Sender<V>,
    mailbox: mpsc::Receiver<DriverCommand<C, Waiter>>,
    calls: Arc<Semaphore>,
    closing: watch::Receiver<()>,
    log_note: fn(&Note),
    interrupted: bool,
}

impl<C, V, E, T, A, P> runner::Runner<Event<C, T, A>, Action<V, E, T, A, P>>
    for Runner<C, V>
where
    C: Send,
    V: Send,
    E: From<Failure<io::Error, P>> + Send,
    T: Send,
    A: Send,
    P: Send,
{
    async fn next(&mut self) -> Option<Event<C, T, A>> {
        Some(tokio::select! {
            biased;
            line = read(self.process.as_mut()), if !self.interrupted => Input::Read(line),
            command = self.mailbox.recv() => match command {
                Some(call @ DriverCommand::Call(_)) => {
                    self.calls.add_permits(1);
                    Input::Command(call)
                }
                Some(shutdown) => Input::Command(shutdown),
                None => Input::Closed,
            },
        })
    }

    async fn perform(
        &mut self,
        effect: Action<V, E, T, A, P>,
    ) -> Result<Option<Event<C, T, A>>, End> {
        match effect {
            Effect::Write { line, then } => {
                let outcome = self.write(&line).await;
                Ok(Some(Input::Wrote { then, outcome }))
            }
            Effect::Emit(event) => {
                self.emit(event).await;
                Ok(None)
            }
            Effect::Log(note) => {
                (self.log_note)(&note);
                Ok(None)
            }
            Effect::Reply(reply, answer) => {
                let _ = reply.send(answer.map_err(E::from));
                Ok(None)
            }
            Effect::Report(waiter, exit) => {
                let _ = waiter.send(exit);
                Ok(None)
            }
            Effect::Stop(then) => {
                let (exit, stderr) = match self.process.take() {
                    Some(process) => process.stop().await,
                    None => {
                        (Exit::Unknown(String::from("already stopped")), None)
                    }
                };
                Ok(Some(Input::Stopped { exit, stderr, then }))
            }
            Effect::End => Err(End),
        }
    }
}

impl<C, V> Runner<C, V> {
    async fn emit(&mut self, event: V) {
        if let Err(mpsc::error::TrySendError::Full(queued)) =
            self.events.try_send(event)
            && !self.interrupted
        {
            tokio::select! {
                biased;
                _ = self.closing.changed() => self.interrupted = true,
                _ = self.events.send(queued) => {}
            }
        }
    }

    async fn write(&mut self, line: &str) -> io::Result<()> {
        let process = self.process.as_mut().ok_or(io::ErrorKind::BrokenPipe)?;
        if self.interrupted {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        // A partially written line cannot be retried. Report the interrupted
        // write before delivering shutdown and never write to the pipe again.
        tokio::select! {
            biased;
            result = process::write_line(&mut process.stdin, line) => result,
            _ = self.closing.changed() => {
                self.interrupted = true;
                Err(io::ErrorKind::BrokenPipe.into())
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests;
