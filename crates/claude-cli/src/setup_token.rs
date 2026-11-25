use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::time::Duration;

use agents::Var;
use agents::{Credential, SignInCode};
use claude::TerminalLine;
use install::Home;
use portable_pty::{
    Child, CommandBuilder, MasterPty, PtySize, native_pty_system,
};
use runtime::Launch;
use tokio::sync::{mpsc, oneshot};

const OPEN_TIMEOUT: Duration = Duration::from_secs(30);
const CODE_TIMEOUT: Duration = Duration::from_mins(10);
const INK_PASTE_TO_ENTER_PAUSE: Duration = Duration::from_millis(40);
const UNWRAPPED_OAUTH_URL_COLUMNS: u16 = 1400;
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_LINE: u64 = 16 * 1024;

const START_FAILED: &str = "Claude could not start setup-token.";
const STOPPED: &str = "Claude stopped before completing setup-token.";
const INTERRUPTED: &str = "Claude setup-token sign-in was canceled.";

pub async fn sign_in(
    launch: &Launch,
    label: String,
    show: oneshot::Sender<String>,
    mut entered: oneshot::Receiver<SignInCode>,
) -> Result<Credential, String> {
    let (_terminal, writer, mut lines) =
        Terminal::open(launch).map_err(String::from)?;
    let link = find(&mut lines, OPEN_TIMEOUT, |line| {
        claude::setup_token_link(line.text())
    })
    .await?;
    show.send(link).map_err(|_| String::from(INTERRUPTED))?;
    let SignInCode(returned) = tokio::time::timeout(CODE_TIMEOUT, async {
        loop {
            tokio::select! {
                code = &mut entered => break code.map_err(|_| INTERRUPTED),
                frame = lines.recv() => {
                    if frame.is_none() {
                        break Err(STOPPED);
                    }
                }
            }
        }
    })
    .await
    .map_err(|_| String::from("Claude setup-token sign-in timed out."))?
    .map_err(String::from)?;
    let code = claude::setup_token_code(&returned)
        .ok_or_else(|| String::from("Claude requires a valid return code."))?;
    let _writer =
        tokio::time::timeout(REPLY_TIMEOUT, submit(writer, String::from(code)))
            .await
            .map_err(|_| String::from("Claude setup-token sign-in timed out."))?
            .map_err(String::from)?;
    let mut output = claude::SetupTokenOutput::default();
    let token =
        find(&mut lines, REPLY_TIMEOUT, |line| output.line(line)).await?;
    Ok(claude::setup_token_credential(label, token))
}

async fn find(
    lines: &mut mpsc::Receiver<TerminalLine>,
    timeout: Duration,
    mut parse: impl FnMut(&TerminalLine) -> Option<String>,
) -> Result<String, String> {
    tokio::time::timeout(timeout, async {
        while let Some(line) = lines.recv().await {
            if let Some(value) = parse(&line) {
                return Ok(value);
            }
        }
        Err(String::from(STOPPED))
    })
    .await
    .map_err(|_| String::from("Claude setup-token sign-in timed out."))?
}

struct Terminal {
    child: Option<Box<dyn Child + Send + Sync>>,
    master: Option<Box<dyn MasterPty + Send>>,
}

type TerminalIo = (
    Terminal,
    Box<dyn Write + Send>,
    mpsc::Receiver<TerminalLine>,
);

impl Terminal {
    fn open(launch: &Launch) -> Result<TerminalIo, &'static str> {
        let Home::Private(home) = &launch.home else {
            return Err(START_FAILED);
        };
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 32,
                cols: UNWRAPPED_OAUTH_URL_COLUMNS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|_| START_FAILED)?;
        let reader = pty.master.try_clone_reader().map_err(|_| START_FAILED)?;
        let writer = pty.master.take_writer().map_err(|_| START_FAILED)?;
        let child = pty
            .slave
            .spawn_command(command(&launch.path, home))
            .map_err(|_| START_FAILED)?;
        drop(pty.slave);

        let terminal = Self {
            child: Some(child),
            master: Some(pty.master),
        };
        let (tx, rx) = mpsc::channel(16);
        tokio::task::spawn_blocking(move || read_lines(reader, &tx));
        Ok((terminal, writer, rx))
    }
}

async fn submit(
    mut writer: Box<dyn Write + Send>,
    code: String,
) -> Result<Box<dyn Write + Send>, &'static str> {
    tokio::task::spawn_blocking(move || {
        writer.write_all(code.as_bytes())?;
        writer.flush()?;
        std::thread::sleep(INK_PASTE_TO_ENTER_PAUSE);
        writer.write_all(b"\r")?;
        writer.flush()?;
        Ok::<_, io::Error>(writer)
    })
    .await
    .map_err(|_| STOPPED)?
    .map_err(|_| STOPPED)
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let master = self.master.take();
            tokio::task::spawn_blocking(move || {
                if !matches!(child.try_wait(), Ok(Some(_))) {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                drop(master);
            });
        }
    }
}

fn read_lines(terminal_output: impl Read, tx: &mpsc::Sender<TerminalLine>) {
    let mut reader = BufReader::new(terminal_output);
    loop {
        let mut bytes = Vec::new();
        let read = reader.by_ref().take(MAX_LINE).read_until(b'\n', &mut bytes);
        let Some(line) = read.ok().and_then(|_| TerminalLine::whole(&bytes))
        else {
            break;
        };
        if tx.blocking_send(line).is_err() {
            break;
        }
    }
}

fn command(path: &Path, home: &Path) -> CommandBuilder {
    let run = claude::setup_token_run();
    let mut command = CommandBuilder::new(path);
    command.args(run.args);
    command.cwd(home);
    for var in run.env {
        match var {
            Var::Set { name, value } => command.env(name, value),
            Var::Remove(name) => command.env_remove(name),
            Var::Home(name) => command.env(name, home),
        }
    }
    command
}
