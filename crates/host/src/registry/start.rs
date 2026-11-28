use alloc::boxed::Box;
use alloc::string::ToString;
use core::ops::ControlFlow::{self, Break, Continue};

use crate::log::Event;
use crate::registry::InstanceId;
use crate::session::{
    EventKind, SessionDirectory, SessionId, SessionRefusal, SessionResult,
    StartAt,
};
use crate::workspace::{Snapshot, Workspace};
use agents::{ClientName, Effort, ModelId, Models, Provider};
use home::{Home, HomeRefusal, IssueKey, IssueName, ProjectId, tracker::Ids};

use super::{Launch, Registry, on_issue, unbriefed, unprepared};
use crate::prelude::*;
use crate::session::Choice;
use vcs::Error as GitError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub project: Option<ProjectId>,
    pub directory: SessionDirectory,
    pub on: Option<home::issue::Start>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InHome {
    Project(ProjectId),
    Issue(IssueName),
}

pub fn place_without_home(at: StartAt) -> Result<Placement, InHome> {
    match at {
        StartAt::Directory(directory) => Ok(Placement {
            project: None,
            directory,
            on: None,
        }),
        StartAt::Project(id) => Err(InHome::Project(id)),
        StartAt::Issue(name) => Err(InHome::Issue(name)),
    }
}

pub fn place_in_home(
    at: InHome,
    home: Option<&Home>,
    ids: &Ids,
) -> Result<Placement, HomeRefusal> {
    match at {
        InHome::Project(id) => {
            home::first_checkout(home, id).map(|checkout| Placement {
                project: Some(id),
                directory: checkout.into(),
                on: None,
            })
        }
        InHome::Issue(name) => {
            let id = ids.key(&name).map_err(|_| HomeRefusal::UnknownIssue)?;
            home::issue::start(home, id).map(|(project, on)| Placement {
                project: Some(project),
                directory: on.checkout.clone().into(),
                on: Some(on),
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartRequest {
    pub instance: InstanceId,
    pub model: Option<ModelId>,
    pub effort: Option<Effort>,
    pub by: ClientName,
}

#[derive(Debug)]
pub enum StartEffect<Starter, Handle> {
    ReadyBranch(home::issue::Start),
    CheckDirectory(SessionDirectory),
    CreateWorkspace {
        directory: SessionDirectory,
        base: Option<String>,
    },
    Snapshot {
        session: SessionId,
        workspace: Workspace,
    },
    Record {
        session: SessionId,
        before: Vec<EventKind>,
        seed: Box<[EventKind; 2]>,
    },
    Spawn {
        session: SessionId,
        directory: SessionDirectory,
        choice: Choice,
        provider: Provider,
        agent: (Starter, Models),
        history: Box<[Event; 2]>,
    },
    Brief {
        session: SessionId,
        handle: Handle,
        text: String,
        by: ClientName,
    },
}

pub type StartDecision<Starter, Handle> =
    ControlFlow<StartEnd, StartStep<Starter, Handle>>;

#[derive(Debug)]
pub struct StartStep<Starter, Handle> {
    pub effect: StartEffect<Starter, Handle>,
    pub next: StartSession<Starter, Handle>,
}

#[derive(Debug)]
pub enum StartEnd {
    Finished(StartOutcome),
    Stopped,
}

#[derive(Debug)]
pub struct StartOutcome {
    pub progress: Option<(IssueKey, ClientName)>,
    pub result: SessionResult,
}

#[derive(Debug)]
pub enum StartReport<Handle> {
    Branch(Result<(), SessionRefusal>),
    Directory(bool),
    Workspace(SessionId, Result<Workspace, GitError>),
    Snapshot(Result<String, GitError>),
    Recorded(Box<[Event; 2]>),
    Spawned(Handle),
    BriefQueued,
    Stopped,
}

#[derive(Debug)]
enum Phase<Starter, Handle> {
    Branch,
    Directory,
    Workspace(Launch<Starter>),
    Snapshot(SessionId, Launch<Starter>, Workspace),
    Record(SessionId, Launch<Starter>),
    Spawn(SessionId),
    Brief(SessionId, Handle),
}

#[derive(Debug)]
pub struct StartSession<Starter, Handle> {
    request: StartRequest,
    placed: Placement,
    phase: Phase<Starter, Handle>,
}

type Planned<Starter, Handle> =
    (Phase<Starter, Handle>, StartEffect<Starter, Handle>);

impl StartRequest {
    pub fn placed<Starter, Handle>(
        self,
        result: Result<Placement, HomeRefusal>,
        history: &[Event],
    ) -> StartDecision<Starter, Handle> {
        let placed = match result {
            Ok(placement) => placement,
            Err(reason) => return refuse(SessionRefusal::Home(reason)),
        };
        let (phase, effect) = match &placed.on {
            Some(on) => {
                if let Some(session) =
                    on_issue(history, &on.issue, &self.instance)
                {
                    return refuse(SessionRefusal::IssueTaken { session });
                }
                (Phase::Branch, StartEffect::ReadyBranch(on.clone()))
            }
            None => (
                Phase::Directory,
                StartEffect::CheckDirectory(placed.directory.clone()),
            ),
        };
        Continue(StartStep {
            effect,
            next: StartSession {
                request: self,
                placed,
                phase,
            },
        })
    }
}

impl<Starter: Clone, Handle: Clone> StartSession<Starter, Handle> {
    pub fn report<ReplyTo>(
        self,
        registry: &mut Registry<Starter, Handle, ReplyTo>,
        report: StartReport<Handle>,
    ) -> StartDecision<Starter, Handle> {
        let Self {
            request,
            placed,
            phase: previous,
        } = self;
        let (phase, effect) = match (previous, report) {
            (Phase::Branch, StartReport::Branch(readied)) => {
                if let Err(reason) = readied {
                    return refuse(reason);
                }
                (
                    Phase::Directory,
                    StartEffect::CheckDirectory(placed.directory.clone()),
                )
            }
            (Phase::Directory, StartReport::Directory(exists)) => {
                match workspace_creation(registry, &request, &placed, exists) {
                    Ok(next) => next,
                    Err(reason) => return refuse(reason),
                }
            }
            (
                Phase::Workspace(launch),
                StartReport::Workspace(session, created),
            ) => match created {
                Ok(workspace) => snapshotting(session, launch, workspace),
                Err(error) => return refuse(unprepared(error)),
            },
            (
                Phase::Snapshot(session, launch, workspace),
                StartReport::Snapshot(snapshot),
            ) => match snapshot {
                Ok(commit) => {
                    let effect = opening(
                        &request, &placed, session, &launch, workspace, commit,
                    );
                    (Phase::Record(session, launch), effect)
                }
                Err(error) => {
                    return refuse(SessionRefusal::WorkspaceFailure {
                        reason: error.to_string(),
                    });
                }
            },
            (
                Phase::Record(session, launch),
                StartReport::Recorded(history),
            ) => (
                Phase::Spawn(session),
                StartEffect::Spawn {
                    session,
                    directory: placed.directory.clone(),
                    choice: launch.choice,
                    provider: request.instance.provider.clone(),
                    agent: (launch.start, launch.offered),
                    history,
                },
            ),
            (Phase::Spawn(session), StartReport::Spawned(handle)) => {
                match &placed.on {
                    Some(on) => (
                        Phase::Brief(session, handle.clone()),
                        StartEffect::Brief {
                            session,
                            handle,
                            text: on.brief.clone(),
                            by: request.by.clone(),
                        },
                    ),
                    None => {
                        return admitted(
                            registry, request, placed, session, handle,
                        );
                    }
                }
            }
            (Phase::Brief(session, handle), StartReport::BriefQueued) => {
                return admitted(registry, request, placed, session, handle);
            }
            (_, _) => return Break(StartEnd::Stopped),
        };
        Continue(StartStep {
            effect,
            next: Self {
                request,
                placed,
                phase,
            },
        })
    }
}

fn workspace_creation<Starter: Clone, Handle: Clone, ReplyTo>(
    registry: &Registry<Starter, Handle, ReplyTo>,
    request: &StartRequest,
    placed: &Placement,
    exists: bool,
) -> Result<Planned<Starter, Handle>, SessionRefusal> {
    let launch = registry.launch_session(
        &request.instance,
        exists,
        request.model.clone(),
        request.effort,
    )?;
    Ok((
        Phase::Workspace(launch),
        StartEffect::CreateWorkspace {
            directory: placed.directory.clone(),
            base: placed.on.as_ref().map(|on| on.branch.clone()),
        },
    ))
}

fn snapshotting<Starter, Handle>(
    session: SessionId,
    launch: Launch<Starter>,
    workspace: Workspace,
) -> Planned<Starter, Handle> {
    (
        Phase::Snapshot(session, launch, workspace.clone()),
        StartEffect::Snapshot { session, workspace },
    )
}

fn opening<Starter, Handle>(
    request: &StartRequest,
    placed: &Placement,
    session: SessionId,
    launch: &Launch<Starter>,
    workspace: Workspace,
    commit: String,
) -> StartEffect<Starter, Handle> {
    let created = EventKind::SessionCreated {
        instance: request.instance.clone(),
        directory: placed.directory.clone(),
        by: request.by.clone(),
        model: launch.choice.model.clone(),
        effort: launch.choice.effort,
    };
    let attached = placed.on.as_ref().map(|on| EventKind::OnIssue {
        issue: on.issue.clone(),
    });
    let snapshot = Snapshot {
        turn_start: None,
        commit,
    };
    StartEffect::Record {
        session,
        before: core::iter::once(created).chain(attached).collect(),
        seed: Box::new([
            EventKind::WorkspaceCreated { workspace },
            EventKind::SnapshotRecorded { snapshot },
        ]),
    }
}

fn admitted<Starter: Clone, Handle: Clone, ReplyTo>(
    registry: &mut Registry<Starter, Handle, ReplyTo>,
    request: StartRequest,
    placed: Placement,
    session: SessionId,
    handle: Handle,
) -> StartDecision<Starter, Handle> {
    registry.admit(session, request.instance, handle);
    Break(StartEnd::Finished(StartOutcome {
        progress: placed.on.map(|on| (on.issue, request.by)),
        result: SessionResult::Started { session },
    }))
}

const fn refuse<Starter, Handle>(
    reason: SessionRefusal,
) -> StartDecision<Starter, Handle> {
    Break(StartEnd::Finished(StartOutcome {
        progress: None,
        result: SessionResult::Refused(reason),
    }))
}

#[must_use]
pub fn brief_sent(answer: Option<&SessionResult>) -> Option<EventKind> {
    unbriefed(answer).map(|message| EventKind::Error { message })
}
