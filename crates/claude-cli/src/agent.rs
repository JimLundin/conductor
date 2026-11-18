//! Claude's runtime: one task per session's agent runs the
//! [`claude::runtime`] machine on a [`Runtime`]'s far end, starting the CLI
//! and making the calls it decides on. The agents of one [`starter`], one
//! instance's, share the credential the latest [`Command::Credential`] set.
//!
//! [`Command::Credential`]: agents::runtime::Command::Credential

use std::convert::Infallible;
use std::future::pending;
use std::path::PathBuf;
use std::sync::Arc;

use claude::ClaudeCredential;
use claude::runtime::{Call, Effect, Input, Refusal, Spawn};
use claude::{
    ClaudeEvent, PermissionMode, Resume, SettingSource, VersionPolicy,
};
use install::Home;
use proto::Models;
use runtime::{Plug, Runtime, Starter};
use tokio::sync::{mpsc, watch};

use crate::{ClaudeOptions, ClaudeSession, Error, sandbox};

/// How the host runs Claude.
#[derive(Debug, Clone)]
pub struct ClaudeLaunch {
    pub claude_path: PathBuf,
    pub home: Home,
    /// The credential agents start with until another is set; `None`
    /// leaves the home's own sign-in.
    pub credential: Option<ClaudeCredential>,
    /// Where the CLI starts to list its models.
    pub probe: PathBuf,
}

/// What every agent of one starter shares.
struct Shared {
    launch: ClaudeLaunch,
    sandbox: claude::Sandbox,
    credential: watch::Sender<Option<ClaudeCredential>>,
}

/// Starts Claude agents with `launch`.
#[must_use]
pub fn starter(launch: ClaudeLaunch) -> Starter {
    let sandbox = sandbox();
    if let Some(note) = claude::unprotected(launch.credential.as_ref(), sandbox)
    {
        runtime::log(&note);
    }
    let (credential, _) = watch::channel(launch.credential.clone());
    let shared = Arc::new(Shared {
        launch,
        sandbox,
        credential,
    });
    Starter::new(move || start(&shared))
}

fn start(shared: &Arc<Shared>) -> Runtime {
    let (runtime, plug) = runtime::pair();
    let task = Task {
        machine: claude::runtime::Runtime::new(shared.sandbox),
        shared: Arc::clone(shared),
        agent: None,
        plug,
    };
    tokio::spawn(task.run());
    runtime
}

/// The running CLI and its events.
type Agent = (ClaudeSession, mpsc::Receiver<ClaudeEvent>);

struct Task {
    machine: claude::runtime::Runtime,
    shared: Arc<Shared>,
    agent: Option<Agent>,
    plug: Plug,
}

impl Task {
    /// Runs until the session drops its runtime, which drops the CLI.
    async fn run(mut self) -> Option<Infallible> {
        loop {
            let input = tokio::select! {
                command = self.plug.commands.recv() => Input::Command(command?),
                event = next_event(self.agent.as_mut()) => Input::Event(event),
            };
            self.feed(input).await;
        }
    }

    /// Steps the machine and performs its effects in order, each effect's
    /// own effects before the next.
    async fn feed(&mut self, input: Input) {
        let mut effects = self.machine.step(input);
        effects.reverse();
        while let Some(effect) = effects.pop() {
            let more = self.perform(effect).await;
            effects.extend(more.into_iter().rev());
        }
    }

    async fn perform(&mut self, effect: Effect) -> Vec<Effect> {
        match effect {
            Effect::Spawn(spawn) => {
                let spawned = self.spawn(spawn).await;
                self.machine.step(Input::Spawned(spawned))
            }
            Effect::Call { call, then } => {
                let outcome = self.call(call).await;
                self.machine.step(Input::Called { then, outcome })
            }
            Effect::Shutdown => {
                let stopped = shutdown(self.agent.take()).await;
                self.machine.step(Input::ShutDown(stopped))
            }
            Effect::Probe => {
                let listed = probe(&self.shared).await;
                self.machine.step(Input::Probed(listed))
            }
            Effect::Credential(credential) => {
                self.shared.credential.send_replace(credential);
                Vec::new()
            }
            Effect::Release => {
                self.agent = None;
                Vec::new()
            }
            Effect::Emit(event) => {
                // A session that has gone takes no more events.
                let _ = self.plug.events.send(event).await;
                Vec::new()
            }
            Effect::Log(note) => {
                runtime::log(&note);
                Vec::new()
            }
        }
    }

    /// Starts the CLI in place of the one running, if it starts.
    async fn spawn(
        &mut self,
        spawn: Spawn,
    ) -> Result<claude::SessionId, Refusal> {
        let options = options(&self.shared, spawn);
        let (session, events) = ClaudeSession::spawn(options)
            .await
            .map_err(|error| refusal(&error))?;
        let id = session.id().clone();
        self.agent = Some((session, events));
        Ok(id)
    }

    async fn call(&self, call: Call) -> Result<(), String> {
        let Some((session, _)) = &self.agent else {
            return Err(String::from("the agent is not running"));
        };
        match call {
            Call::SetEffort(effort) => session.set_effort(effort).await,
            Call::SendUser(text) => session.send_user(text).await,
            Call::Interrupt => session.interrupt().await,
            Call::Respond { id, decision } => {
                session.respond(id, decision).await
            }
        }
        .map_err(|error| error.to_string())
    }
}

/// The running CLI's next event. With none running, it waits forever, so
/// the task only takes commands.
async fn next_event(agent: Option<&mut Agent>) -> Option<ClaudeEvent> {
    match agent {
        Some((_, events)) => events.recv().await,
        None => pending().await,
    }
}

/// How a session's CLI starts. The user level of its settings is the
/// host's private home (§6.2), and its transcripts go under the session's
/// own project directory.
fn options(shared: &Shared, spawn: Spawn) -> ClaudeOptions {
    ClaudeOptions {
        claude_path: shared.launch.claude_path.clone(),
        home: shared.launch.home.clone(),
        credential: shared.credential.borrow().clone(),
        cwd: PathBuf::from(spawn.directory.0),
        model: spawn.model,
        effort: spawn.effort,
        permission_mode: PermissionMode::Default,
        resume: spawn.resume,
        project_name: Some(ulid::Ulid::from(spawn.session.0).to_string()),
        setting_sources: Some(Vec::from([
            SettingSource::User,
            SettingSource::Project,
            SettingSource::Local,
        ])),
        stdout_log: None,
        versions: VersionPolicy::AllowUntested,
    }
}

/// Why the CLI didn't start, and the session it found missing.
fn refusal(error: &Error) -> Refusal {
    let missing = match error {
        Error::MissingSession(session) => Some(session.clone()),
        _ => None,
    };
    Refusal {
        message: error.to_string(),
        missing,
    }
}

async fn shutdown(agent: Option<Agent>) -> Result<(), String> {
    match agent {
        Some((session, _)) => session
            .shutdown()
            .await
            .map(drop)
            .map_err(|error| error.to_string()),
        None => Ok(()),
    }
}

/// What a Claude session offers, from its `initialize` response: the CLI
/// is started in the probe directory and stopped again.
async fn probe(shared: &Shared) -> Result<Models, String> {
    let directory = &shared.launch.probe;
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let credential = shared.credential.borrow().clone();
    let (session, _events) = ClaudeSession::spawn(ClaudeOptions {
        claude_path: shared.launch.claude_path.clone(),
        home: shared.launch.home.clone(),
        credential,
        cwd: directory.clone(),
        model: None,
        effort: None,
        permission_mode: PermissionMode::Default,
        resume: Resume::Fresh,
        project_name: None,
        setting_sources: None,
        stdout_log: None,
        versions: VersionPolicy::AllowUntested,
    })
    .await
    .map_err(|error| error.to_string())?;
    let models = session.models().clone();
    let _ = session.shutdown().await;
    Ok(models)
}
