//! Finishing a session on an issue (docs/home.md §4).
//!
//! Whether it can, the summary its agent gives, how its branch lands on
//! the issue's, and what the log says of it. The shell asks the agent,
//! runs `git` and writes the issue's note.

use alloc::format;

use crate::log::{Event, HostSeq};
use crate::session::{EventKind, SessionId, SessionRefusal, SessionResult};
use agents::ClientName;
use agents::Output;
use home::{Home, IssueId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::workspace::{Kept, Workspace};

use crate::assignment::{Assigned, Assignment};
use crate::prelude::*;
use crate::workspace::kept;

/// What the agent of a session being finished is asked, as its last turn.
const SUMMARY: &str = "Conductor is finishing this session. In a few sentences, sum up what you did on the issue and what is left, as a note for whoever picks it up next. Make no more changes.";

/// Why the agent of a finished session stops.
const FINISHED: &str = "the session was finished";

/// A session that can finish, as its log has it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Finishing {
    retry: bool,
    assignment: Assignment,
    workspace: Workspace,
    /// Its agent runs, so it is asked for a summary first.
    running: bool,
}

/// Whether `session` can finish after `history`: it is on an issue, has a
/// workspace, isn't removed, runs no turn, and has done something since
/// it last finished. A session still starting counts as running.
///
/// # Errors
///
/// Why it can't.
fn finishing(
    history: &[Event],
    session: SessionId,
) -> Result<Finishing, SessionRefusal> {
    let mut assigned = Assigned::default();
    let mut workspace = None;
    let mut removed = false;
    let mut running = false;
    let mut turning = false;
    let mut finish = FinishState::Ready;
    for event in history.iter().filter(|event| event.session == session) {
        if let Some(active) = crate::restore::agent_activity(&event.kind) {
            running = active;
        }
        match &event.kind {
            EventKind::WorkspaceCreated { workspace: made } => {
                workspace = Some(made.clone());
            }
            EventKind::SessionRemoved { .. } => removed = true,
            EventKind::UserMessage { .. } => turning = true,
            EventKind::TurnCompleted { .. }
            | EventKind::SessionClosed { .. } => turning = false,
            _ => {}
        }
        finish = finish.after(&event.kind);
        assigned = assigned.after(&event.kind);
    }
    let instance = assigned.instance.ok_or(SessionRefusal::UnknownSession)?;
    let refused = [
        (removed, SessionRefusal::SessionRemoved),
        (turning, SessionRefusal::TurnInProgress),
        (
            matches!(finish, FinishState::Finishing | FinishState::Done),
            SessionRefusal::AlreadyFinished,
        ),
    ];
    if let Some((_, refusal)) = refused.into_iter().find(|(is, _)| *is) {
        return Err(refusal);
    }
    Ok(Finishing {
        retry: finish == FinishState::Retrying,
        assignment: Assignment {
            issue: assigned.issue.ok_or(SessionRefusal::NotOnIssue)?,
            instance,
        },
        workspace: workspace.ok_or_else(|| {
            SessionRefusal::WorkspaceFailure {
                reason: String::from("the session has no workspace to land"),
            }
        })?,
        running,
    })
}

/// The summary for the issue's notes: the session's last reply after
/// `after`, the seq the summary turn started after, or its last reply of
/// all when its agent had stopped.
#[must_use]
fn summary(
    events: &[Event],
    session: SessionId,
    after: HostSeq,
) -> Option<String> {
    events
        .iter()
        .filter(|event| event.session == session && event.seq > after)
        .filter_map(|event| match &event.kind {
            EventKind::Agent(Output::AssistantMessage { text }) => {
                Some(text.clone())
            }
            _ => None,
        })
        .next_back()
        .filter(|text| !text.trim().is_empty())
}

/// Whether the summary turn has ended, by the event the log stored.
#[must_use]
fn turn_ended(event: &Event, session: SessionId) -> bool {
    event.session == session
        && matches!(
            event.kind,
            EventKind::TurnCompleted { .. } | EventKind::SessionClosed { .. }
        )
}

/// What finishing did: the commits that landed, or why none did, and the
/// push of the issue's branch.
#[must_use]
fn finished(
    landed: Result<u64, String>,
    pushed: Option<(String, Result<(), String>)>,
) -> Finished {
    match landed {
        Ok(commits) => Finished::Landed {
            commits,
            kept: kept(pushed),
        },
        Err(reason) => Finished::Failed { reason },
    }
}

/// The commits that landed, for moving the issue on.
#[must_use]
const fn landed(finished: &Finished) -> u64 {
    match finished {
        Finished::Landed { commits, .. } => *commits,
        Finished::Failed { .. } => 0,
    }
}

/// How long the agent has to sum up before finishing gives up, in
/// seconds.
const PATIENCE: u64 = 600;

/// Why finishing landed nothing when the summary turn didn't end in time.
const IMPATIENT: &str =
    "the agent didn't sum up within 10 minutes, so it was interrupted";

/// Why finishing landed nothing when the agent wasn't asked or stopped:
/// a turn got in first.
#[must_use]
fn interrupted(what: &str, answer: Option<&SessionResult>) -> Option<String> {
    match answer {
        Some(SessionResult::Accepted) => None,
        Some(SessionResult::Refused(refusal)) => {
            Some(format!("{what}: {}", refusal.label()))
        }
        Some(other) => Some(format!("{what}: the session answered {other:?}")),
        None => Some(format!("{what}: the session stopped")),
    }
}

/// One finish, advanced by the session actor.
#[derive(Debug)]
pub struct Finish {
    session: SessionId,
    stage: Stage,
}

#[derive(Debug)]
enum Stage {
    Checking(ClientName),
    Asking(Assignment, Workspace, HostSeq),
    Watching(Assignment, Workspace, HostSeq, Option<String>),
    Stopping(Assignment, Workspace, Option<String>),
    Interrupting(Assignment),
    ReadingHome(Assignment, Workspace, Option<String>),
    Readying(Assignment, Workspace, Option<String>, String),
    Landing(Assignment, Workspace, Option<String>, String),
    Pushing(Assignment, Option<String>, u64),
    Noting(Finished),
    Done,
}

/// An outcome of an effect or an event from the summary's subscription.
#[derive(Debug)]
pub enum Input {
    History(Vec<Event>),
    Ordered(Option<SessionResult>),
    Events(Vec<Event>),
    Lagged,
    TimedOut,
    RequestedAgain(Vec<Event>),
    Home(Option<Home>),
    Ready(Result<(), String>),
    Landed(Result<u64, String>),
    Pushed(Option<(String, Result<(), String>)>),
    Noted,
}

/// Work the session actor performs in order.
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    History,
    Emit(EventKind),
    Reply(SessionResult),
    Order(crate::session::Order),
    Watch {
        after: HostSeq,
        seconds: u64,
    },
    Replay(HostSeq),
    NextEvent,
    ReadHome,
    Ready {
        workspace: Workspace,
        branch: String,
    },
    Land {
        workspace: Workspace,
        branch: String,
    },
    Push {
        workspace: Workspace,
        branch: String,
    },
    Note {
        issue: IssueId,
        summary: Option<String>,
        by: ClientName,
        commits: u64,
    },
}

impl Finish {
    #[must_use]
    pub const fn new(session: SessionId, by: ClientName) -> (Self, Effect) {
        (
            Self {
                session,
                stage: Stage::Checking(by),
            },
            Effect::History,
        )
    }

    #[must_use]
    pub fn step(&mut self, input: Input) -> Vec<Effect> {
        if let Input::RequestedAgain(history) = input {
            let reason = finishing(&history, self.session)
                .err()
                .unwrap_or(SessionRefusal::AlreadyFinished);
            return Vec::from([
                Effect::Reply(SessionResult::Refused(reason)),
                Effect::NextEvent,
            ]);
        }
        let prior = core::mem::replace(&mut self.stage, Stage::Done);
        match (prior, input) {
            (Stage::Checking(by), Input::History(history)) => {
                self.check(&history, by)
            }
            (
                Stage::Asking(assignment, workspace, after),
                Input::Ordered(answer),
            ) => self.asked(assignment, workspace, after, answer.as_ref()),
            (
                Stage::Watching(assignment, workspace, after, last),
                Input::Events(events),
            ) => self.observe(assignment, workspace, after, last, &events),
            (
                Stage::Watching(assignment, workspace, after, _),
                Input::Lagged,
            ) => {
                self.stage =
                    Stage::Watching(assignment, workspace, after, None);
                Vec::from([Effect::Replay(after)])
            }
            (Stage::Watching(assignment, ..), Input::TimedOut) => {
                self.stage = Stage::Interrupting(assignment);
                order(crate::session::Order::Interrupt)
            }
            (Stage::Interrupting(assignment), Input::Ordered(_)) => {
                self.failed(&assignment, None, String::from(IMPATIENT))
            }
            (
                Stage::Stopping(assignment, workspace, last),
                Input::Ordered(answer),
            ) => match interrupted("stopping the agent", answer.as_ref()) {
                Some(reason) => self.failed(&assignment, None, reason),
                None => self.read_home(assignment, workspace, last),
            },
            (
                Stage::ReadingHome(assignment, workspace, last),
                Input::Home(home),
            ) => self.ready(assignment, workspace, last, home.as_ref()),
            (
                Stage::Readying(assignment, workspace, last, branch),
                Input::Ready(Ok(())),
            ) => {
                self.stage = Stage::Landing(
                    assignment,
                    workspace.clone(),
                    last,
                    branch.clone(),
                );
                Vec::from([Effect::Land { workspace, branch }])
            }
            (
                Stage::Readying(assignment, _, last, _),
                Input::Ready(Err(reason)),
            )
            | (
                Stage::Landing(assignment, _, last, _),
                Input::Landed(Err(reason)),
            ) => self.failed(&assignment, last, reason),
            (
                Stage::Landing(assignment, workspace, last, branch),
                Input::Landed(Ok(commits)),
            ) => {
                self.stage = Stage::Pushing(assignment, last, commits);
                Vec::from([Effect::Push { workspace, branch }])
            }
            (
                Stage::Pushing(assignment, last, commits),
                Input::Pushed(pushed),
            ) => self.note(&assignment, last, finished(Ok(commits), pushed)),
            (Stage::Noting(finished), Input::Noted) => {
                Vec::from([Effect::Emit(EventKind::SessionFinished {
                    finished,
                })])
            }
            (stage, _) => {
                self.stage = stage;
                Vec::new()
            }
        }
    }

    fn check(&mut self, history: &[Event], by: ClientName) -> Vec<Effect> {
        let Finishing {
            retry,
            assignment,
            workspace,
            running,
        } = match finishing(history, self.session) {
            Ok(finishing) => finishing,
            Err(refusal) => {
                return Vec::from([Effect::Reply(SessionResult::Refused(
                    refusal,
                ))]);
            }
        };
        let mut effects = Vec::new();
        if !retry {
            effects.push(Effect::Emit(EventKind::FinishStarted));
        }
        effects.push(Effect::Reply(SessionResult::Accepted));
        if running {
            let after = history.last().map_or(HostSeq(0), |event| event.seq);
            self.stage = Stage::Asking(assignment, workspace, after);
            effects.extend(order(crate::session::Order::Send {
                text: String::from(SUMMARY),
                by,
            }));
        } else {
            let last = summary(history, self.session, HostSeq(0));
            effects.extend(self.read_home(assignment, workspace, last));
        }
        effects
    }

    fn asked(
        &mut self,
        assignment: Assignment,
        workspace: Workspace,
        after: HostSeq,
        answer: Option<&SessionResult>,
    ) -> Vec<Effect> {
        if let Some(reason) = interrupted("asking for a summary", answer) {
            return self.failed(&assignment, None, reason);
        }
        self.stage = Stage::Watching(assignment, workspace, after, None);
        Vec::from([Effect::Watch {
            after,
            seconds: PATIENCE,
        }])
    }

    fn ready(
        &mut self,
        assignment: Assignment,
        workspace: Workspace,
        last: Option<String>,
        home: Option<&Home>,
    ) -> Vec<Effect> {
        let branch = home::issue::branch(home, assignment.issue);
        self.stage = Stage::Readying(
            assignment,
            workspace.clone(),
            last,
            branch.clone(),
        );
        Vec::from([Effect::Ready { workspace, branch }])
    }

    fn observe(
        &mut self,
        assignment: Assignment,
        workspace: Workspace,
        after: HostSeq,
        mut last: Option<String>,
        events: &[Event],
    ) -> Vec<Effect> {
        for event in events
            .iter()
            .filter(|event| event.session == self.session && event.seq > after)
        {
            if turn_ended(event, self.session) {
                self.stage = Stage::Stopping(assignment, workspace, last);
                return order(crate::session::Order::Stop(String::from(
                    FINISHED,
                )));
            }
            if let EventKind::Agent(Output::AssistantMessage { text }) =
                &event.kind
            {
                last = (!text.trim().is_empty()).then(|| text.clone());
            }
        }
        self.stage = Stage::Watching(assignment, workspace, after, last);
        Vec::from([Effect::NextEvent])
    }

    fn read_home(
        &mut self,
        assignment: Assignment,
        workspace: Workspace,
        last: Option<String>,
    ) -> Vec<Effect> {
        self.stage = Stage::ReadingHome(assignment, workspace, last);
        Vec::from([Effect::ReadHome])
    }

    fn failed(
        &mut self,
        assignment: &Assignment,
        last: Option<String>,
        reason: String,
    ) -> Vec<Effect> {
        self.note(assignment, last, finished(Err(reason), None))
    }

    fn note(
        &mut self,
        assignment: &Assignment,
        summary: Option<String>,
        finished: Finished,
    ) -> Vec<Effect> {
        let commits = landed(&finished);
        self.stage = Stage::Noting(finished);
        Vec::from([Effect::Note {
            issue: assignment.issue,
            summary,
            by: assignment.signer(),
            commits,
        }])
    }
}

fn order(order: crate::session::Order) -> Vec<Effect> {
    Vec::from([Effect::Order(order)])
}

/// Where a session on an issue stands with finishing, as its events say:
/// a message after a finish makes it ready again, while its own summary
/// turn doesn't.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FinishState {
    #[default]
    Ready,
    Finishing,
    Retrying,
    Done,
}

impl FinishState {
    /// The state after `kind`.
    #[must_use]
    pub const fn after(self, kind: &EventKind) -> Self {
        match (self, kind) {
            (_, EventKind::FinishStarted) => Self::Finishing,
            (Self::Finishing, EventKind::FinishInterrupted) => Self::Retrying,
            (_, EventKind::SessionFinished { .. }) => Self::Done,
            (Self::Done, EventKind::UserMessage { .. }) => Self::Ready,
            (state, _) => state,
        }
    }
}

/// How finishing a session on an issue went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Finished {
    /// The session's branch landed on the issue's with this many commits
    /// the issue's lacked, and the issue's branch was kept as `kept` says.
    Landed { commits: u64, kept: Kept },
    /// Nothing landed, for this reason; the session's branch stays.
    Failed { reason: String },
}

#[cfg(test)]
mod tests;
