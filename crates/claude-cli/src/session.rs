//! The Claude handle and handshake over the shared line-JSON process runner.

use std::path::{Path, PathBuf};
use std::time::Duration;

use claude::ClaudeCredential;
use proto::{Effort, Models};
use serde_json::Value;

use agents::Exit;
use install::Home;
use install::line_json::{self, Handle, Options, StderrPolicy};
use tokio::process::Command;
use tokio::sync::mpsc;

use claude::{
    CallRequest, ClaudeEvent, ControlCall, Failure, Launch, PermissionDecision,
    PermissionMode, PermissionRequestId, Problem, Resume, Sandbox, Session,
    SessionId, SettingSource, UnsupportedVersion, VersionPolicy, models,
};

const CONTROL_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct ClaudeOptions {
    pub claude_path: PathBuf,
    pub home: Home,
    /// Passed in the environment; `None` leaves the home's own sign-in.
    pub credential: Option<ClaudeCredential>,
    pub cwd: PathBuf,
    pub model: Option<String>,
    /// `None` leaves the model's default effort.
    pub effort: Option<Effort>,
    pub permission_mode: PermissionMode,
    pub resume: Resume,
    /// Conductor's stable session ULID isolates its transcript directory.
    pub project_name: Option<String>,
    /// `None` leaves the CLI default, which loads user, project and local settings.
    pub setting_sources: Option<Vec<SettingSource>>,
    /// Appended to the CLI's system prompt.
    pub instructions: Option<String>,
    /// Append every raw stdout line to this file.
    pub stdout_log: Option<PathBuf>,
    pub versions: VersionPolicy,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("claude i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not encode a message for claude: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("the claude process has exited")]
    Exited,
    /// The CLI exited before it answered `initialize`, saying why.
    #[error("claude exited: {0}")]
    Refused(String),
    /// Only the CLI's exact missing-transcript reason for the requested resume id.
    #[error("claude exited: No conversation found with session ID: {}", .0.0)]
    MissingSession(SessionId),
    #[error("claude rejected {subtype}: {message}")]
    Control { subtype: String, message: String },
    #[error("claude did not answer {0} within {CONTROL_TIMEOUT:?}")]
    Timeout(String),
    #[error(
        "claude {found} is not supported (supported: {supported}); set VersionPolicy::AllowUntested to run it anyway"
    )]
    UnsupportedVersion { found: String, supported: String },
    #[error(
        "claude did not report its version, so it can't be checked; set VersionPolicy::AllowUntested to run it anyway"
    )]
    UnreportedVersion,
    #[error("no open permission request with id {}", .0.0)]
    UnknownPermissionRequest(PermissionRequestId),
}

impl From<Failure<std::io::Error>> for Error {
    fn from(failure: Failure<std::io::Error>) -> Self {
        match failure {
            Failure::Exited => Self::Exited,
            Failure::Refused(reason) => Self::Refused(reason),
            Failure::Io(error) => Self::Io(error),
            Failure::Json(error) => Self::Encode(error),
            Failure::Protocol(Problem::Control { subtype, message }) => {
                Self::Control { subtype, message }
            }
            Failure::Protocol(Problem::UnknownPermissionRequest(id)) => {
                Self::UnknownPermissionRequest(id)
            }
        }
    }
}

impl From<UnsupportedVersion> for Error {
    fn from(unsupported: UnsupportedVersion) -> Self {
        match unsupported {
            UnsupportedVersion::Outside { found, supported } => {
                Self::UnsupportedVersion { found, supported }
            }
            UnsupportedVersion::Unreported => Self::UnreportedVersion,
        }
    }
}

type CommandMsg = CallRequest<line_json::Reply<Error>>;

pub struct ClaudeSession {
    commands: Handle<CommandMsg>,
    id: SessionId,
    /// What the `initialize` response lists.
    models: Models,
}

/// A session whose process runs, before it has answered `initialize`.
struct Starting {
    session: ClaudeSession,
    events: mpsc::Receiver<ClaudeEvent>,
    last_stderr: line_json::Stderr,
}

impl ClaudeSession {
    /// Starts `claude` and completes the `initialize` handshake. Refuses a CLI
    /// version outside the supported range unless `VersionPolicy::AllowUntested`.
    ///
    /// # Errors
    /// Fails when the CLI cannot be started, exits or refuses before it
    /// answers `initialize`, or reports a version the policy refuses.
    pub async fn spawn(
        opts: ClaudeOptions,
    ) -> Result<(Self, mpsc::Receiver<ClaudeEvent>), Error> {
        let id = opts
            .resume
            .target()
            .cloned()
            .unwrap_or_else(|| SessionId(uuid::Uuid::new_v4().to_string()));
        let mut starting = start(&opts, id)?;
        let initialize = starting.initialize(&opts.resume).await?;
        starting.admit(initialize, opts.versions).await
    }

    /// The models and efforts this CLI offers.
    #[must_use]
    pub const fn models(&self) -> &Models {
        &self.models
    }

    /// The id `--resume` takes, which the CLI prints in every `init`.
    #[must_use]
    pub const fn id(&self) -> &SessionId {
        &self.id
    }

    /// Changes the effort of the CLI's later API requests, in this process
    /// using `apply_flag_settings`.
    ///
    /// # Errors
    /// Fails when the CLI has exited, does not answer in time or rejects
    /// the request.
    pub async fn set_effort(&self, effort: Effort) -> Result<(), Error> {
        let _ = self.control(ControlCall::SetEffort(effort)).await?;
        Ok(())
    }

    /// # Errors
    /// Fails when the CLI has exited or the message cannot be written.
    pub async fn send_user(&self, text: String) -> Result<(), Error> {
        let _ = self
            .commands
            .ask(|reply| CommandMsg::SendUser { text, reply })
            .await
            .map_err(|_| Error::Exited)??;
        Ok(())
    }

    /// Resolves once the CLI acknowledges; the turn's `result` follows as an event.
    ///
    /// # Errors
    /// Fails when the CLI has exited, does not answer in time or rejects
    /// the request.
    pub async fn interrupt(&self) -> Result<(), Error> {
        let _ = self.control(ControlCall::Interrupt).await?;
        Ok(())
    }

    /// # Errors
    /// Fails when the CLI has exited, no request with `id` is open, or the
    /// answer cannot be written.
    pub async fn respond(
        &self,
        id: PermissionRequestId,
        decision: PermissionDecision,
    ) -> Result<(), Error> {
        let _ = self
            .commands
            .ask(|reply| CommandMsg::Respond {
                id,
                decision,
                reply,
            })
            .await
            .map_err(|_| Error::Exited)??;
        Ok(())
    }

    /// Closes stdin, waits briefly for a clean exit, then kills.
    ///
    /// # Errors
    /// Fails when the session's actor has already stopped.
    pub async fn shutdown(self) -> Result<Exit, Error> {
        self.commands.shutdown().await.map_err(|_| Error::Exited)
    }

    async fn control(&self, call: ControlCall) -> Result<Value, Error> {
        tokio::time::timeout(
            CONTROL_TIMEOUT,
            self.commands
                .ask(|reply| CommandMsg::Control { call, reply }),
        )
        .await
        .map_err(|_| Error::Timeout(String::from(call.subtype())))?
        .map_err(|_| Error::Exited)?
    }
}

/// Starts the process and its actor.
fn start(opts: &ClaudeOptions, id: SessionId) -> Result<Starting, Error> {
    let stdout_log = line_json::stdout_log(opts.stdout_log.as_deref())?;
    let mut driver = Session::default();
    let started = line_json::start(
        command(opts, &id, sandbox(&opts.cwd)),
        Options {
            stdout_log,
            stderr: StderrPolicy::Last,
            log_stderr: |line| tracing::warn!(target: "claude::stderr", "{line}"),
            log_note: runtime::log,
            commands: 32,
            events: 1024,
        },
        move |input| driver.step(input),
    )?;
    Ok(Starting {
        session: ClaudeSession {
            commands: started.handle,
            id,
            models: Models::none(),
        },
        events: started.events,
        last_stderr: started.stderr,
    })
}

impl Starting {
    /// The `initialize` answer, or why the CLI exited before it gave one.
    async fn initialize(&mut self, resume: &Resume) -> Result<Value, Error> {
        let answer = self.session.control(ControlCall::Initialize).await;
        self.unless_exited(answer, resume).await
    }

    async fn unless_exited(
        &mut self,
        answer: Result<Value, Error>,
        resume: &Resume,
    ) -> Result<Value, Error> {
        match answer {
            Err(Error::Exited) => Err(self.refusal(resume).await),
            received => received,
        }
    }

    /// Why a CLI exited before it answered `initialize`: the `errors` of the
    /// `result` it printed, else its last stderr line.
    async fn refusal(&mut self, resume: &Resume) -> Error {
        let events = std::iter::from_fn(|| self.events.try_recv().ok())
            .collect::<Vec<ClaudeEvent>>();
        let reason =
            reason(claude::reported_refusal(events), &mut self.last_stderr)
                .await;
        resume
            .missing(&reason)
            .map_or(Error::Refused(reason), Error::MissingSession)
    }

    async fn admit(
        self,
        initialize: Value,
        policy: VersionPolicy,
    ) -> Result<(ClaudeSession, mpsc::Receiver<ClaudeEvent>), Error> {
        let session = ClaudeSession {
            models: models(&initialize),
            ..self.session
        };
        match claude::admit(initialize, policy) {
            Ok(warning) => {
                warning.iter().for_each(runtime::log);
                Ok((session, self.events))
            }
            Err(unsupported) => Err(session.refuse(unsupported).await),
        }
    }
}

impl ClaudeSession {
    async fn refuse(self, unsupported: UnsupportedVersion) -> Error {
        self.shutdown()
            .await
            .err()
            .unwrap_or_else(|| Error::from(unsupported))
    }
}

async fn reason(
    reported: Option<String>,
    last_stderr: &mut line_json::Stderr,
) -> String {
    match reported {
        Some(reason) => reason,
        None => last_stderr
            .last(CONTROL_TIMEOUT)
            .await
            .unwrap_or_else(|| String::from("it printed no reason")),
    }
}

/// Whether Claude's command sandbox can run here (`claude::sandbox`).
#[must_use]
pub fn sandbox(cwd: &Path) -> Sandbox {
    let kernel_release = if cfg!(target_os = "linux") {
        std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .unwrap_or_default()
    } else {
        String::new()
    };
    claude::sandbox(std::env::consts::OS, sandbox_tools(cwd), &kernel_release)
}

#[cfg(target_os = "linux")]
fn sandbox_tools(cwd: &Path) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    claude::SANDBOX_TOOLS.iter().all(|tool| {
        std::env::split_paths(&path)
            .any(|dir| executable(&cwd.join(dir).join(tool)))
    })
}

#[cfg(not(target_os = "linux"))]
const fn sandbox_tools(_: &Path) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn executable(path: &Path) -> bool {
    use rustix::fs::{Access, AtFlags, CWD, accessat};

    path.is_file()
        && accessat(CWD, path, Access::EXEC_OK, AtFlags::EACCESS).is_ok()
}

fn command(opts: &ClaudeOptions, id: &SessionId, sandbox: Sandbox) -> Command {
    let launch = Launch {
        home: opts.home.kind(),
        credential: opts.credential.clone(),
        project_name: opts.project_name.clone(),
        permission_mode: opts.permission_mode,
        resume: opts.resume.clone(),
        id: id.clone(),
        model: opts.model.clone(),
        effort: opts.effort,
        setting_sources: opts.setting_sources.clone(),
        instructions: opts.instructions.clone(),
        sandbox,
    };
    let mut cmd = install::process::command(
        &opts.claude_path,
        &opts.home,
        claude::session(&launch),
    );
    cmd.current_dir(&opts.cwd);
    cmd
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use proto::Effort;

    use super::{
        ClaudeOptions, PermissionMode, Resume, Sandbox, SessionId,
        SettingSource, VersionPolicy, command,
    };
    use install::Home;

    /// The variables the resume test checks, as the command sets them.
    fn checked_env(
        std: &std::process::Command,
    ) -> Result<Vec<(&str, Option<&str>)>, &'static str> {
        std.get_envs()
            .filter_map(|(key, setting)| match (key.to_str(), setting) {
                (
                    Some(
                        "CLAUDECODE"
                        | "CLAUDE_CODE_OAUTH_TOKEN"
                        | "CLAUDE_CODE_USE_BEDROCK"
                        | "CLAUDE_CODE_USE_VERTEX"
                        | "CLAUDE_CODE_USE_FOUNDRY"
                        | "ANTHROPIC_BASE_URL"
                        | "CLAUDE_CODE_SESSION_ID",
                    ),
                    None,
                ) => Some((key, None)),
                (
                    Some(
                        "CLAUDE_CONFIG_DIR"
                        | "CLAUDE_CODE_PROJECT_DIR_NAME"
                        | "DISABLE_UPDATES",
                    ),
                    Some(value),
                ) => Some((key, value.to_str())),
                _ => None,
            })
            .map(|(key, value)| key.to_str().map(|name| (name, value)))
            .collect::<Option<Vec<(&str, Option<&str>)>>>()
            .ok_or("a variable is not UTF-8")
    }

    #[test]
    fn resumed_session_argv() -> Result<(), Box<dyn std::error::Error>> {
        let cmd = command(
            &ClaudeOptions {
                instructions: None,
                claude_path: "/opt/claude".into(),
                home: Home::Private(PathBuf::from(
                    "/srv/conductor/home/claude",
                )),
                credential: None,
                cwd: "/tmp".into(),
                model: Some(String::from("haiku")),
                effort: Some(Effort::XHigh),
                permission_mode: PermissionMode::Default,
                resume: Resume::Session(SessionId(String::from("0b1c"))),
                project_name: Some(String::from("01JCDZ6Q0Y3K7V8M2N4P5R6S7T")),
                setting_sources: Some(Vec::from([
                    SettingSource::Project,
                    SettingSource::Local,
                ])),
                stdout_log: None,
                versions: VersionPolicy::SupportedOnly,
            },
            &SessionId(String::from("0b1c")),
            Sandbox::Missing,
        );
        let std = cmd.as_std();
        let args = std
            .get_args()
            .map(|a| a.to_str())
            .collect::<Option<Vec<&str>>>()
            .ok_or("an argument is not UTF-8")?;
        assert_eq!(std.get_program(), "/opt/claude");
        assert_eq!(std.get_current_dir(), Some(Path::new("/tmp")));
        assert_eq!(
            args,
            [
                "--print",
                "--verbose",
                "--output-format",
                "stream-json",
                "--input-format",
                "stream-json",
                "--permission-mode",
                "default",
                "--resume=0b1c",
                "--model",
                "haiku",
                "--effort",
                "xhigh",
                "--setting-sources",
                "project,local",
                "--replay-user-messages",
                "--include-partial-messages",
                "--thinking-display",
                "summarized",
                "--permission-prompt-tool",
                "stdio",
            ]
        );
        assert_eq!(
            checked_env(std)?,
            [
                ("ANTHROPIC_BASE_URL", None),
                ("CLAUDECODE", None),
                ("CLAUDE_CODE_OAUTH_TOKEN", None),
                (
                    "CLAUDE_CODE_PROJECT_DIR_NAME",
                    Some("01JCDZ6Q0Y3K7V8M2N4P5R6S7T")
                ),
                ("CLAUDE_CODE_SESSION_ID", None),
                ("CLAUDE_CODE_USE_BEDROCK", None),
                ("CLAUDE_CODE_USE_FOUNDRY", None),
                ("CLAUDE_CODE_USE_VERTEX", None),
                ("CLAUDE_CONFIG_DIR", Some("/srv/conductor/home/claude")),
                ("DISABLE_UPDATES", Some("1")),
            ]
        );
        Ok(())
    }
}

#[cfg(test)]
#[cfg(unix)]
mod replay;
