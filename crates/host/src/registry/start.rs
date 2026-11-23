use alloc::boxed::Box;
use alloc::string::ToString;
use core::ops::ControlFlow::{self, Break, Continue};

use home::{CheckoutLocation, Home, HomeRefusal, IssueId};
use proto::{
    ClientName, Effort, Event, EventKind, HostPath, InstanceId, ModelId,
    Models, Provider, SessionId, SessionRefusal, SessionResult, Snapshot,
    StartAt, Workspace,
};

use super::{Launch, Registry, on_issue, unbriefed, unprepared};
use crate::prelude::*;
use crate::session::Choice;
use crate::workspace::GitError;

/// Where a session starts, and the issue it works on, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub directory: HostPath,
    pub on: Option<home::issue::Start>,
}

/// The directory a session starts in: the one given, or the first
/// checkout of a project or an issue's project. The session's project
/// follows from its worktree's repository, so it isn't kept apart.
///
/// # Errors
///
/// The host has no home, its home lists no such project or issue, or the
/// project has no checkout on the host.
pub fn place(
    at: StartAt,
    home: Option<&Home>,
) -> Result<Placement, HomeRefusal> {
    match at {
        StartAt::Directory(directory) => Ok(Placement {
            directory,
            on: None,
        }),
        StartAt::Project(id) => {
            home::first_checkout(home, id).map(|checkout| Placement {
                directory: path_of(checkout),
                on: None,
            })
        }
        StartAt::Issue(id) => {
            home::issue::start(home, id).map(|on| Placement {
                directory: path_of(on.checkout.clone()),
                on: Some(on),
            })
        }
    }
}

/// The host path a checkout is at.
#[must_use]
pub fn path_of(checkout: CheckoutLocation) -> HostPath {
    HostPath(checkout.0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartRequest {
    pub instance: InstanceId,
    pub model: Option<ModelId>,
    pub effort: Option<Effort>,
    pub by: ClientName,
}

#[derive(Debug)]
pub enum StartEffect<S, H> {
    ReadyBranch(home::issue::Start),
    CheckDirectory(HostPath),
    CreateWorkspace {
        directory: HostPath,
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
        directory: HostPath,
        choice: Choice,
        provider: Provider,
        agent: (S, Models),
        history: Box<[Event; 2]>,
    },
    Brief {
        session: SessionId,
        handle: H,
        text: String,
        by: ClientName,
    },
}

pub type StartDecision<S, H> = ControlFlow<StartEnd, StartStep<S, H>>;

#[derive(Debug)]
pub struct StartStep<S, H> {
    pub effect: StartEffect<S, H>,
    pub next: StartSession<S, H>,
}

#[derive(Debug)]
pub enum StartEnd {
    Finished(StartOutcome),
    Stopped,
}

/// The completion waits for the home progress update before replying.
#[derive(Debug)]
pub struct StartOutcome {
    pub progress: Option<(IssueId, ClientName)>,
    pub result: SessionResult,
}

#[derive(Debug)]
pub enum StartReport<H> {
    Branch(Result<(), SessionRefusal>),
    Directory(bool),
    Workspace(SessionId, Result<Workspace, GitError>),
    Snapshot(Result<String, GitError>),
    Recorded(Box<[Event; 2]>),
    Spawned(H),
    BriefQueued,
    Stopped,
}

#[derive(Debug)]
enum Phase<S, H> {
    Branch,
    Directory,
    Workspace(Launch<S>),
    Snapshot(SessionId, Launch<S>, Workspace),
    Record(SessionId, Launch<S>),
    Spawn(SessionId),
    Brief(SessionId, H),
}

#[derive(Debug)]
pub struct StartSession<S, H> {
    request: StartRequest,
    placed: Placement,
    phase: Phase<S, H>,
}

type Planned<S, H> = (Phase<S, H>, StartEffect<S, H>);

impl StartRequest {
    pub fn placed<S, H>(
        self,
        result: Result<Placement, HomeRefusal>,
        history: &[Event],
    ) -> StartDecision<S, H> {
        let placed = match result {
            Ok(placement) => placement,
            Err(reason) => return rejected(SessionRefusal::Home(reason)),
        };
        let (phase, effect) = match &placed.on {
            Some(on) => {
                if let Some(session) =
                    on_issue(history, on.issue, &self.instance)
                {
                    return rejected(SessionRefusal::IssueTaken { session });
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

impl<S: Clone, H: Clone> StartSession<S, H> {
    pub fn report<R>(
        self,
        registry: &mut Registry<S, H, R>,
        report: StartReport<H>,
    ) -> StartDecision<S, H> {
        let Self {
            request,
            placed,
            phase: previous,
        } = self;
        let (phase, effect) = match (previous, report) {
            (Phase::Branch, StartReport::Branch(readied)) => {
                if let Err(reason) = readied {
                    return rejected(reason);
                }
                (
                    Phase::Directory,
                    StartEffect::CheckDirectory(placed.directory.clone()),
                )
            }
            (Phase::Directory, StartReport::Directory(exists)) => {
                match workspace_creation(registry, &request, &placed, exists) {
                    Ok(next) => next,
                    Err(reason) => return rejected(reason),
                }
            }
            (
                Phase::Workspace(launch),
                StartReport::Workspace(session, created),
            ) => match created {
                Ok(workspace) => snapshotting(session, launch, workspace),
                Err(error) => return rejected(unprepared(error)),
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
                    return rejected(SessionRefusal::WorkspaceFailure {
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

fn workspace_creation<S: Clone, H: Clone, R>(
    registry: &Registry<S, H, R>,
    request: &StartRequest,
    placed: &Placement,
    exists: bool,
) -> Result<Planned<S, H>, SessionRefusal> {
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

fn snapshotting<S, H>(
    session: SessionId,
    launch: Launch<S>,
    workspace: Workspace,
) -> Planned<S, H> {
    (
        Phase::Snapshot(session, launch, workspace.clone()),
        StartEffect::Snapshot { session, workspace },
    )
}

fn opening<S, H>(
    request: &StartRequest,
    placed: &Placement,
    session: SessionId,
    launch: &Launch<S>,
    workspace: Workspace,
    commit: String,
) -> StartEffect<S, H> {
    let created = EventKind::SessionCreated {
        instance: request.instance.clone(),
        directory: placed.directory.clone(),
        by: request.by.clone(),
        model: launch.choice.model.clone(),
        effort: launch.choice.effort,
    };
    let attached = placed
        .on
        .as_ref()
        .map(|on| EventKind::OnIssue { issue: on.issue });
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

fn admitted<S: Clone, H: Clone, R>(
    registry: &mut Registry<S, H, R>,
    request: StartRequest,
    placed: Placement,
    session: SessionId,
    handle: H,
) -> StartDecision<S, H> {
    registry.admit(session, request.instance, handle);
    Break(StartEnd::Finished(StartOutcome {
        progress: placed.on.map(|on| (on.issue, request.by)),
        result: SessionResult::Started { session },
    }))
}

const fn rejected<S, H>(reason: SessionRefusal) -> StartDecision<S, H> {
    Break(StartEnd::Finished(StartOutcome {
        progress: None,
        result: SessionResult::Refused(reason),
    }))
}

#[must_use]
pub fn brief_sent(answer: Option<&SessionResult>) -> Option<EventKind> {
    unbriefed(answer).map(|message| EventKind::Error { message })
}
