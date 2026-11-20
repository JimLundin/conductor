use alloc::boxed::Box;
use alloc::string::ToString;
use core::ops::ControlFlow::{self, Break, Continue};

use home::Placement;
use proto::{
    Checkpoint, CheckpointId, ClientName, CommandResult, Effort, Event,
    EventKind, HistoryAnchor, HostPath, InstanceId, IssueId, ModelId, Models,
    Provider, Rejection, SessionId, UndoAvailability, Workspace,
};

use super::{Launch, Registry, on_issue, unbriefed, unprepared};
use crate::prelude::*;
use crate::session::Choice;
use crate::workspace::GitError;

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
        id: CheckpointId,
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
    pub result: CommandResult,
}

#[derive(Debug)]
pub enum StartReport<H> {
    Branch(Result<(), Rejection>),
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

const INITIAL: CheckpointId = CheckpointId(0);

impl StartRequest {
    pub fn placed<S, H>(
        self,
        result: Result<Placement, Rejection>,
        history: &[Event],
    ) -> StartDecision<S, H> {
        let placed = match result {
            Ok(placement) => placement,
            Err(reason) => return rejected(reason),
        };
        let (phase, effect) = match &placed.on {
            Some(on) => {
                if let Some(session) =
                    on_issue(history, on.issue, &self.instance)
                {
                    return rejected(Rejection::IssueTaken { session });
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
                    return rejected(Rejection::WorkspaceFailure {
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
                    provider: launch.provider,
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
) -> Result<Planned<S, H>, Rejection> {
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
        StartEffect::Snapshot {
            session,
            workspace,
            id: INITIAL,
        },
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
        provider: launch.provider,
        directory: placed.directory.clone(),
        by: request.by.clone(),
        model: launch.choice.model.clone(),
        effort: launch.choice.effort,
    };
    let attached = placed
        .on
        .as_ref()
        .map(|on| EventKind::OnIssue { issue: on.issue });
    let checkpoint = Checkpoint {
        id: INITIAL,
        parent: None,
        turn_start: None,
        commit,
        history: HistoryAnchor::Fresh,
        undo: UndoAvailability::Unavailable {
            reason: String::from("initial checkpoint has no turn"),
        },
    };
    StartEffect::Record {
        session,
        before: core::iter::once(created).chain(attached).collect(),
        seed: Box::new([
            EventKind::WorkspaceCreated { workspace },
            EventKind::CheckpointCreated { checkpoint },
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
        result: CommandResult::Started { session },
    }))
}

const fn rejected<S, H>(reason: Rejection) -> StartDecision<S, H> {
    Break(StartEnd::Finished(StartOutcome {
        progress: None,
        result: CommandResult::Rejected(reason),
    }))
}

#[must_use]
pub fn brief_sent(answer: Option<&CommandResult>) -> Option<EventKind> {
    unbriefed(answer).map(|message| EventKind::Error { message })
}
