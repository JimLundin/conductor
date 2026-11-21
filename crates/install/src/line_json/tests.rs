use std::convert::Infallible;
use std::io;
use std::time::Duration;

use agents::Exit;
use agents::driver::{After, Command as DriverCommand, Effect, Failure, Input};
use serde_json::Value;
use tokio::process::Command;
use tokio::sync::Notify;
use tokio::time::timeout;

use super::{Action, Event, Options, Reply, Started, StderrPolicy, start};

const DEADLINE: Duration = Duration::from_secs(15);
static WRITE_STARTED: Notify = Notify::const_new();

#[derive(Debug, PartialEq, Eq)]
enum Observed {
    Line(String),
    Exited(Exit, Option<String>),
}

struct Call {
    line: String,
    reply: Reply<Failure<io::Error, Infallible>>,
}

type WriteFailure = Failure<io::Error, Infallible>;
type DriverEffect =
    Action<Observed, WriteFailure, Reply<WriteFailure>, Infallible, Infallible>;

fn fixture(
    script: &str,
    stderr: StderrPolicy,
    log_stderr: fn(&str),
) -> io::Result<Started<Call, Observed>> {
    let mut child = Command::new("/bin/sh");
    child.args(["-c", script]);
    let mut exited = None;
    start(
        child,
        Options {
            processes: crate::process::Processes::default(),
            stdout_log: None,
            stderr,
            log_stderr,
            log_note: |_| {},
            commands: 1,
            events: 4,
        },
        move |input| step(&mut exited, input),
    )
}

fn step(
    exited: &mut Option<Exit>,
    input: Event<Call, Reply<WriteFailure>, Infallible>,
) -> Vec<DriverEffect> {
    match input {
        Input::Read(Ok(Some(line))) => vec![Effect::Emit(Observed::Line(line))],
        Input::Read(Ok(None) | Err(_)) => After::Nothing
            .stop(exited.as_ref())
            .into_iter()
            .flatten()
            .collect(),
        Input::Command(DriverCommand::Call(Call { line, reply })) => {
            vec![Effect::Write { line, then: reply }]
        }
        Input::Command(DriverCommand::Shutdown(waiter)) => {
            After::Shutdown(waiter)
                .stop(exited.as_ref())
                .into_iter()
                .flatten()
                .collect()
        }
        Input::Closed => After::End
            .stop(exited.as_ref())
            .into_iter()
            .flatten()
            .collect(),
        Input::Wrote { then, outcome } => {
            vec![Effect::Reply(
                then,
                outcome.map(|()| Value::Null).map_err(Failure::Io),
            )]
        }
        Input::Stopped { exit, stderr, then } => {
            *exited = Some(exit.clone());
            std::iter::once(Effect::Emit(Observed::Exited(
                exit.clone(),
                stderr,
            )))
            .chain(then.complete(&exit).into_iter().flatten())
            .collect()
        }
    }
}

#[tokio::test]
async fn exit_delivers_unterminated_stdout_line_before_status()
-> Result<(), Box<dyn std::error::Error>> {
    let mut running =
        fixture("printf '{'; exit 7", StderrPolicy::Last, |_| {})?;
    assert_eq!(
        timeout(DEADLINE, running.events.recv()).await?,
        Some(Observed::Line(String::from("{")))
    );
    assert_eq!(
        timeout(DEADLINE, running.events.recv()).await?,
        Some(Observed::Exited(
            Exit::Status(String::from("exit status: 7")),
            None
        ))
    );
    Ok(())
}

#[tokio::test]
async fn exit_with_only_stderr_keeps_last_nonempty_line()
-> Result<(), Box<dyn std::error::Error>> {
    let mut running = fixture(
        "printf 'first\nreason\n \n' >&2; exit 2",
        StderrPolicy::LastNonempty(DEADLINE),
        |_| {},
    )?;
    assert_eq!(
        timeout(DEADLINE, running.events.recv()).await?,
        Some(Observed::Exited(
            Exit::Status(String::from("exit status: 2")),
            Some(String::from("reason"))
        ))
    );
    Ok(())
}

#[tokio::test]
async fn shutdown_interrupts_full_stdin_and_reaps_child()
-> Result<(), Box<dyn std::error::Error>> {
    let Started { handle, .. } = fixture(
        "head -c 1 >/dev/null; printf 'stdin started\n' >&2; exec sleep 30",
        StderrPolicy::Last,
        |line| {
            if line == "stdin started" {
                WRITE_STARTED.notify_one();
            }
        },
    )?;
    let writer = handle.clone();
    let writing = tokio::spawn(async move {
        writer
            .ask(|reply| Call {
                line: "x".repeat(1024 * 1024),
                reply,
            })
            .await
    });
    timeout(DEADLINE, WRITE_STARTED.notified()).await?;
    assert!(!writing.is_finished());
    let (queued, exit) = timeout(DEADLINE, async {
        tokio::join!(
            biased;
            handle.ask(|reply| Call { line: String::from("{}"), reply }),
            handle.shutdown()
        )
    })
    .await?;
    assert!(
        matches!(exit?, Exit::Status(status) if status.starts_with("signal: 9"))
    );
    for answer in [writing.await??, queued?] {
        assert!(
            matches!(answer, Err(Failure::Io(error)) if error.kind() == io::ErrorKind::BrokenPipe)
        );
    }
    Ok(())
}
