use std::path::PathBuf;
use std::sync::Arc;

use agents::Configuration;
use agents::runtime::NOT_RUNNING;
use agents::{Credential, Models, SignInCode};
use claude::ClaudeCredential;
use claude::runtime::{Call, Effect, Input, Refusal, Spawn};
use claude::{
    ClaudeEvent, PermissionMode, Resume, SettingSource, VersionPolicy,
};
use runtime::agent::{self, Agent, Performed};
use runtime::{
    Configuring, CredentialSigningIn, Instance, Launch, ProviderInstance,
    Runtime,
};
use tokio::sync::{mpsc, oneshot, watch};

use crate::session::{ClaudeOptions, ClaudeSession, Error, sandbox};

#[derive(Debug)]
struct ClaudeInstance {
    launch: Launch,
    credential: watch::Sender<Option<ClaudeCredential>>,
}

pub fn instance(launch: Launch) -> Instance {
    Arc::new(ClaudeInstance {
        launch,
        credential: watch::channel(None).0,
    })
}

impl ProviderInstance for ClaudeInstance {
    fn configure(&self, given: Option<Credential>) -> Configuring<'_> {
        Box::pin(async move {
            let credential =
                given.as_ref().map(claude::credential).transpose()?;
            if let Some(note) = claude::unprotected(
                credential.as_ref(),
                sandbox(&self.launch.work),
            ) {
                runtime::log(&note);
            }
            self.credential.send_replace(credential.clone());
            Ok(Configuration {
                named: None,
                models: probe(&self.launch, credential).await,
            })
        })
    }

    fn credential_sign_in(
        &self,
        kind: &str,
        account_label: String,
        link: oneshot::Sender<String>,
        code: oneshot::Receiver<SignInCode>,
    ) -> Option<CredentialSigningIn<'_>> {
        (kind == claude::SETUP_TOKEN).then(|| {
            let signing_in: CredentialSigningIn<'_> =
                Box::pin(crate::setup_token::sign_in(
                    &self.launch,
                    account_label,
                    link,
                    code,
                ));
            signing_in
        })
    }

    fn start(self: Arc<Self>) -> Runtime {
        let mut machine = claude::runtime::Runtime::new();
        agent::start(
            Task {
                shared: self,
                agent: None,
            },
            move |input| machine.step(input),
            Input::Command,
            Input::Event,
        )
    }
}

type Session = (ClaudeSession, mpsc::Receiver<ClaudeEvent>);

struct Task {
    shared: Arc<ClaudeInstance>,
    agent: Option<Session>,
}

impl Agent<Input, Effect> for Task {
    type Event = ClaudeEvent;

    fn events(&mut self) -> Option<&mut mpsc::Receiver<Self::Event>> {
        self.agent.as_mut().map(|(_, events)| events)
    }

    fn release(&mut self) {
        self.agent = None;
    }

    async fn perform(&mut self, effect: Effect) -> Performed<Input> {
        let feedback = match effect {
            Effect::Spawn(spawn) => Input::Spawned(self.spawn(spawn).await),
            Effect::Call { call, then } => Input::Called {
                then,
                outcome: self.call(call).await,
            },
            Effect::Shutdown => {
                Input::ShutDown(shutdown(self.agent.take()).await)
            }
            Effect::Runtime(report) => return Performed::Report(report),
        };
        Performed::Feedback(feedback)
    }
}

impl Task {
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
            return Err(String::from(NOT_RUNNING));
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

fn options(shared: &ClaudeInstance, spawn: Spawn) -> ClaudeOptions {
    ClaudeOptions {
        claude_path: shared.launch.path.clone(),
        home: shared.launch.home.clone(),
        credential: shared.credential.borrow().clone(),
        cwd: PathBuf::from(spawn.directory.0),
        model: spawn.model,
        effort: spawn.effort,
        permission_mode: PermissionMode::Default,
        resume: spawn.resume,
        instructions: spawn.instructions,
        project_name: Some(spawn.session),
        setting_sources: Some(Vec::from([
            SettingSource::User,
            SettingSource::Project,
            SettingSource::Local,
        ])),
        stdout_log: None,
        versions: VersionPolicy::AllowUntested,
    }
}

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

async fn shutdown(agent: Option<Session>) -> Result<(), String> {
    match agent {
        Some((session, _)) => session
            .shutdown()
            .await
            .map(drop)
            .map_err(|error| error.to_string()),
        None => Ok(()),
    }
}

async fn probe(
    launch: &Launch,
    credential: Option<ClaudeCredential>,
) -> Result<Models, String> {
    let directory = &launch.work;
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let (session, _events) = ClaudeSession::spawn(ClaudeOptions {
        claude_path: launch.path.clone(),
        home: launch.home.clone(),
        credential,
        cwd: directory.clone(),
        model: None,
        effort: None,
        permission_mode: PermissionMode::Default,
        resume: Resume::Fresh,
        instructions: None,
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
