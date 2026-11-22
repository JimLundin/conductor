//! One session's decisions, as a state machine: its turn, its open
//! requests, its effort, its tasks, its checkpoints and the phases of an
//! undo.
//!
//! `daemon` owns the agent, the log and the workspace. It turns what they do
//! into an [`Input`], steps the session, and performs each [`Effect`] in
//! order. A [`Call`] comes last among the effects of a step, and its outcome
//! is the next input, so the session finishes one order or agent step before
//! it takes the next.
//!
//! The session speaks to its agent only in the runtime protocol
//! (`agents::runtime`): each agent call is a [`Command`], and its
//! [`Outcome`] is the next input. Which provider runs the agent is the
//! runtime's business.
//!
//! The session outlives its agent. When the agent's process exits, the
//! session is closed and keeps its conversation's id, and a `Start` order
//! starts the agent again on it.

use core::mem;

use crate::checkpoints::{Book, Recovery, Undo, UndoPlan};
pub use crate::checkpoints::{Compared, SnapshotPlan};
use crate::prelude::*;
use agents::runtime::{Anchoring, Command, Outcome, Start, Started};
use agents::{NativeRequest, NativeTurn, Reply, Step};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use proto::{
    ChecklistEntry, CheckpointDiff, CheckpointId, ClientName, CloseReason,
    CommandResult, Decision, Effort, Event, EventKind, HistoryAnchor, HostPath,
    HostSeq, Kept, ModelId, Models, Rejection, RequestId, Resolution,
    SessionId, SubagentActivity, SubagentEvent, SubagentId, ToolCallId,
    ToolKind, ToolOutcome, ToolPhase, TurnOutcome, Workspace,
};

/// The model and effort a session runs with, chosen when it starts; a
/// `SetEffort` changes the effort. `None` leaves the CLI's default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub model: Option<ModelId>,
    pub effort: Option<Effort>,
}

/// The model and effort a session asked for, checked against what the
/// provider offers.
///
/// No model is the default. An effort must be one the
/// model lists, unless the host knows nothing of the model (a default the
/// host was given, or one a credential names, that the CLI didn't list).
///
/// # Errors
///
/// Says why when the host doesn't offer the model or the model doesn't
/// take the effort.
pub fn choose(
    models: &Models,
    model: Option<ModelId>,
    effort: Option<Effort>,
) -> Result<Choice, String> {
    let chosen = model.map_or_else(
        || Ok(models.default.clone()),
        |asked| {
            models
                .find(&asked)
                .map(|choice| Some(choice.id.clone()))
                .ok_or_else(|| {
                    format!("this host doesn't offer the model {}", asked.0)
                })
        },
    )?;
    match effort {
        Some(asked) => {
            takes(models, chosen.as_ref(), asked).map(|taken| Choice {
                model: chosen,
                effort: Some(taken),
            })
        }
        None => Ok(Choice {
            model: chosen,
            effort: None,
        }),
    }
}

/// `effort`, if `model` (`None`: the default) takes it: one of the efforts
/// the host lists for it, or any effort when the host doesn't list the
/// model.
fn takes(
    models: &Models,
    model: Option<&ModelId>,
    effort: Effort,
) -> Result<Effort, String> {
    model
        .or(models.default.as_ref())
        .and_then(|id| models.find(id))
        .map_or(Ok(effort), |choice| {
            choice
                .efforts
                .iter()
                .find(|&&offered| offered == effort)
                .copied()
                .ok_or_else(|| {
                    format!(
                        "{} doesn't take the effort {}",
                        choice.name,
                        effort.wire()
                    )
                })
        })
}

/// A command for one session, already routed to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Order {
    Send {
        text: String,
        by: ClientName,
    },
    Interrupt,
    Diff(CheckpointId),
    /// Undoes the turns from `checkpoint` on, restarting the agent if it
    /// must. A conversation forked for it takes `fork` as its id.
    Undo {
        checkpoint: CheckpointId,
        fork: String,
    },
    Reply {
        request: RequestId,
        reply: Reply,
        by: ClientName,
    },
    /// Starts the agent, resuming the conversation if the session has one:
    /// when the session starts, and on `Continue`. `offered` is what the
    /// provider offers now.
    Start {
        offered: Models,
    },
    /// Sets the effort of later turns, if the session's model in these
    /// models takes it. The agent gets it when the next turn starts.
    Effort(Effort, Models),
    /// Stops the agent between turns and removes the session's workspace,
    /// keeping its branch. The session takes no order after it.
    Remove,
    /// Stops the agent between turns, closing the session for this reason.
    Stop(String),
}

/// Why the agent of a session being removed stops.
const REMOVED: &str = "the session was removed";

/// What happened, as the shell saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Order(Order),
    Step(Step),
    /// The outcome of the [`Call::Agent`] command.
    Agent(Outcome),
    /// The outcome of [`Call::Record`].
    Recorded(HostSeq),
    /// The commit [`Call::Snapshot`] made.
    Snapshotted(Result<String, String>),
    Diffed(Result<CheckpointDiff, String>),
    /// The log's events, for [`Call::History`].
    History(Vec<Event>),
    /// Whether the directory of [`Call::Exists`] can be read.
    Exists(bool),
    /// The outcome of a workspace call that answers nothing else.
    Done(Result<(), String>),
    /// Where [`Call::Remove`] kept the branch.
    Removed(Result<Kept, String>),
}

/// What the shell does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Append the event to the log.
    Emit(EventKind),
    /// Save the conversation the session resumes after the host restarts.
    Conversation(String),
    /// Commit what the log staged, before the workspace or the agent
    /// changes.
    Barrier,
    Trace(&'static str),
    /// Answer the order.
    Reply(CommandResult),
    Call(Call),
}

/// An effect whose outcome is the next [`Input`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// Append the event to the log and answer its seq.
    Record(EventKind),
    /// Snapshot the worktree as `checkpoint`, after `previous`'s commit.
    Snapshot {
        workspace: Workspace,
        checkpoint: CheckpointId,
        previous: Option<String>,
    },
    /// Check that the worktree can be restored to `commit`.
    Preflight {
        workspace: Workspace,
        commit: String,
    },
    Restore {
        workspace: Workspace,
        commit: String,
    },
    Diff(Box<Compared>),
    History,
    Exists(HostPath),
    /// Remove the workspace by [`crate::workspace::Plan::remove`], keeping its
    /// branch.
    Remove(Workspace),
    /// Send the agent this command; its outcome is [`Input::Agent`].
    Agent(Command),
}

/// Where an agent starts: the history to restore, else the saved
/// conversation.
type At = (Option<HistoryAnchor>, Option<String>);

/// The agent as the session knows it: how its history is anchored, its
/// conversation and the effort it runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Agent {
    /// No agent runs: it exited, the host stopped it, or it never started.
    Down,
    Up {
        anchoring: Anchoring,
        conversation: String,
        effort: Option<Effort>,
    },
}

impl Agent {
    fn conversation(&self) -> Option<String> {
        match self {
            Self::Up { conversation, .. } => Some(conversation.clone()),
            Self::Down => None,
        }
    }

    const fn effort(&self) -> Option<Effort> {
        match self {
            Self::Up { effort, .. } => *effort,
            Self::Down => None,
        }
    }
}

/// An interrupt that arrived before the agent began the turn. Sent then, it
/// would be lost (docs/architecture.md §5.1), so it waits for `TurnStarted`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Held {
    Nothing,
    Interrupt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Turn {
    Idle,
    /// The message is sent and the agent has not begun the turn yet.
    Starting(Held),
    Running(NativeTurn),
}

/// First decision wins (architecture §8). A resolved request keeps its
/// resolution, so a late answer is told who made the first.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    Open { native: NativeRequest, asked: Asked },
    Resolved(Resolution),
}

impl Pending {
    const fn native(&self) -> Option<&NativeRequest> {
        match self {
            Self::Open { native, .. } => Some(native),
            Self::Resolved(_) => None,
        }
    }
}

/// What an open request waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Asked {
    /// A decision on tool call `call`. `for_session` is what approving for
    /// the session allows, if the agent offers it.
    Approval {
        agent: Option<SubagentId>,
        call: ToolCallId,
        for_session: Option<String>,
    },
    /// One answer for each of `count` questions.
    Question {
        agent: Option<SubagentId>,
        count: usize,
    },
}

impl Asked {
    const fn agent(&self) -> Option<&SubagentId> {
        match self {
            Self::Approval { agent, .. } | Self::Question { agent, .. } => {
                agent.as_ref()
            }
        }
    }
}

/// The call the session waits on, and what follows it.
enum Flow {
    Idle,
    Saving(Saving),
    /// The user's message is being logged; `text` goes to the agent next.
    Recording(String),
    /// The message is being sent, changing the agent's effort to this.
    Sending(Option<Effort>),
    /// An agent call whose failure is logged as an error, then the reply.
    Noting(Option<CommandResult>),
    Diffing,
    /// The agent is starting, for this.
    Starting(Launched),
    Preflighting(Undo),
    /// Finding the directory to restore the agent's history in.
    Locating(Undoing, HistoryAnchor),
    /// The agent stops, to start again at the history.
    Restarting(Undoing, HostPath, HistoryAnchor),
    Listing(Revert),
    Reverting(Revert),
    Checking(Revert),
    Reconciling(Revert),
    /// The agent restored in another directory stops.
    Leaving(Undoing, HistoryAnchor),
    Retasking(Undoing),
    Restoring(Undoing),
    /// The agent stops, for the session to be removed.
    Stopping,
    Removing,
    /// The agent stops, and the session closes for this reason.
    Halting(String),
}

struct Saving {
    plan: SnapshotPlan,
    then: Saved,
}

/// What a snapshot was taken for.
enum Saved {
    Start(Models),
    Continue,
    Send(String, ClientName),
    TurnEnded(TurnOutcome),
    Closed(CloseReason),
}

/// What an agent was started for.
enum Launched {
    Ready,
    Revert(Revert),
    Reconcile(Revert),
    Restart(Undoing, HostPath, HistoryAnchor),
    Complete(Undoing),
}

struct Undoing {
    undo: Undo,
}

/// A conversation anchored at its turns being reverted to before `turn`,
/// in `directory`.
struct Revert {
    undoing: Undoing,
    directory: HostPath,
    conversation: String,
    turn: String,
}

pub struct Session {
    id: SessionId,
    directory: HostPath,
    choice: Choice,
    agent: Agent,
    /// The conversation a start resumes, once a turn has started in it.
    /// Before that the CLI has written no history to resume.
    conversation: Option<String>,
    turn: Turn,
    requests: BTreeMap<RequestId, Pending>,
    last_request: RequestId,
    book: Book,
    turn_start: Option<HostSeq>,
    /// Tool calls the user denied, whose failed result means `Declined`.
    declined: Vec<(Option<SubagentId>, ToolCallId)>,
    tasks: Vec<ChecklistEntry>,
    subagent_tasks: BTreeMap<SubagentId, Vec<ChecklistEntry>>,
    removed: bool,
    flow: Flow,
}

impl Session {
    /// The session `history` logged, with no agent running. A request
    /// still open in it was the gone agent's, so it counts as withdrawn.
    #[must_use]
    pub fn restore(
        id: SessionId,
        directory: HostPath,
        choice: Choice,
        conversation: Option<String>,
        history: &[Event],
    ) -> Self {
        let book = Book::replay(history);
        Self {
            id,
            directory: book
                .workspace()
                .map_or(directory, |workspace| workspace.directory.clone()),
            choice,
            agent: Agent::Down,
            book,
            turn_start: None,
            conversation,
            turn: Turn::Idle,
            requests: replay_requests(history),
            last_request: history
                .iter()
                .filter_map(|event| event.kind.opened_request())
                .max()
                .unwrap_or(RequestId(0)),
            declined: Vec::new(),
            tasks: last_tasks(history.iter()),
            subagent_tasks: history
                .iter()
                .filter_map(|entry| match &entry.kind {
                    EventKind::Subagent {
                        agent,
                        event: SubagentEvent::ChecklistChanged { tasks },
                    } => Some((agent.clone(), tasks.clone())),
                    _ => None,
                })
                .collect(),
            removed: history.iter().any(|event| {
                matches!(event.kind, EventKind::SessionRemoved { .. })
            }),
            flow: Flow::Idle,
        }
    }

    #[must_use]
    pub fn pending_move_notice(&self, history: &[Event]) -> Option<HostPath> {
        self.book
            .workspace()
            .filter(|_| crate::movement::notice_pending(history))
            .map(|workspace| workspace.directory.clone())
    }

    /// Checks that neither the conversation nor the workspace is changing.
    ///
    /// # Errors
    /// The session was removed, is busy, or has no managed workspace.
    pub fn plan_move(&self) -> Result<SnapshotPlan, Rejection> {
        if self.removed {
            return Err(Rejection::SessionRemoved);
        }
        if !matches!((&self.flow, &self.turn), (Flow::Idle, Turn::Idle))
            || self
                .requests
                .values()
                .any(|request| request.native().is_some())
        {
            return Err(Rejection::TurnInProgress);
        }
        if self.book.undo_pending() {
            return Err(Rejection::UndoInProgress);
        }
        self.book.plan_move().ok_or(Rejection::NotGitRepository)
    }

    /// Replays a newly committed move snapshot into the running core.
    pub fn moved_checkpoint(&mut self, event: &Event) {
        self.book.apply(event);
    }

    #[must_use]
    pub fn step(mut self, input: Input) -> (Self, Vec<Effect>) {
        let flow = mem::replace(&mut self.flow, Flow::Idle);
        let effects = self.on(flow, input);
        (self, effects)
    }

    fn on(&mut self, flow: Flow, input: Input) -> Vec<Effect> {
        match (flow, input) {
            (Flow::Idle, Input::Order(order)) => self.order(order),
            (Flow::Idle, Input::Step(step)) => self.agent_step(step),
            (Flow::Saving(saving), Input::Snapshotted(taken)) => {
                self.snapshotted(saving, taken)
            }
            (Flow::Recording(text), Input::Recorded(seq)) => {
                self.recorded(text, seq)
            }
            (Flow::Sending(change), Input::Agent(Outcome::Sent(sent))) => {
                self.sent(change, sent)
            }
            (
                Flow::Noting(then),
                Input::Agent(
                    Outcome::Interrupted(called) | Outcome::Replied(called),
                ),
            ) => noted(called, then),
            (Flow::Diffing, Input::Diffed(diff)) => answer(diffed(diff)),
            (Flow::Starting(then), Input::Agent(Outcome::Started(started))) => {
                self.started(then, started)
            }
            (Flow::Preflighting(order), Input::Done(checked)) => {
                self.preflighted(order, checked)
            }
            (Flow::Locating(undoing, history), Input::Exists(exists)) => {
                self.located(undoing, history, exists)
            }
            (
                Flow::Restarting(undoing, directory, history),
                Input::Agent(Outcome::ShutDown(stopped)),
            ) => self.restart(undoing, directory, history, stopped),
            (Flow::Listing(revert), Input::Agent(Outcome::Turns(listed))) => {
                self.listed(revert, listed)
            }
            (
                Flow::Reverting(revert),
                Input::Agent(Outcome::Reverted(reverted)),
            ) => self.reverted(revert, reverted),
            (Flow::Reverting(revert), Input::Agent(Outcome::RevertUnknown)) => {
                self.launch_revert(revert, Launched::Reconcile)
            }
            (
                Flow::Reconciling(revert) | Flow::Checking(revert),
                Input::Agent(Outcome::Turns(listed)),
            ) => self.checked(revert, listed),
            (
                Flow::Leaving(undoing, history),
                Input::Agent(Outcome::ShutDown(left)),
            ) => self.history_restored(undoing, left.map(|()| history)),
            (Flow::Retasking(undoing), Input::History(events)) => {
                self.retasked(undoing, &events)
            }
            (Flow::Restoring(undoing), Input::Done(restored)) => {
                self.files_restored(undoing, restored)
            }
            (Flow::Stopping, Input::Agent(Outcome::ShutDown(Ok(())))) => join(
                self.closed(CloseReason::Stopped {
                    reason: String::from(REMOVED),
                }),
                self.clean(),
            ),
            (
                Flow::Halting(reason),
                Input::Agent(Outcome::ShutDown(Ok(()))),
            ) => join(
                self.closed(CloseReason::Stopped { reason }),
                [Effect::Reply(CommandResult::Accepted)],
            ),
            (
                Flow::Stopping | Flow::Halting(_),
                Input::Agent(Outcome::ShutDown(Err(reason))),
            ) => join(
                [Effect::Emit(EventKind::Error {
                    message: format!(
                        "the agent did not confirm shutdown: {reason}"
                    ),
                })],
                rejected(Rejection::InstanceUnavailable { reason }),
            ),
            (Flow::Removing, Input::Removed(Ok(kept))) => self.gone(Some(kept)),
            (Flow::Removing, Input::Removed(Err(reason))) => {
                removal_failed(reason)
            }
            (waiting, _) => {
                self.flow = waiting;
                Vec::new()
            }
        }
    }

    fn order(&mut self, order: Order) -> Vec<Effect> {
        if self.removed {
            return rejected(Rejection::SessionRemoved);
        }
        match order {
            Order::Diff(checkpoint) => self.diff(checkpoint),
            Order::Undo { checkpoint, fork } => self.undo(checkpoint, &fork),
            _ if self.book.undo_pending() => {
                rejected(Rejection::UndoInProgress)
            }
            other => self.normal_order(other),
        }
    }

    fn normal_order(&mut self, order: Order) -> Vec<Effect> {
        match (order, &self.turn, &self.agent) {
            (
                Order::Remove | Order::Stop(_),
                Turn::Starting(_) | Turn::Running(_),
                _,
            ) => rejected(Rejection::TurnInProgress),
            (Order::Stop(reason), Turn::Idle, Agent::Up { .. }) => {
                self.flow = Flow::Halting(reason);
                Vec::from([Effect::Call(Call::Agent(Command::Shutdown))])
            }
            (Order::Remove, Turn::Idle, Agent::Down) => self.clean(),
            (Order::Remove, Turn::Idle, Agent::Up { .. }) => {
                self.flow = Flow::Stopping;
                Vec::from([Effect::Call(Call::Agent(Command::Shutdown))])
            }
            (Order::Start { offered }, _, Agent::Down) => {
                self.save(Saved::Start(offered))
            }
            (Order::Start { .. }, Turn::Idle, Agent::Up { .. }) => {
                self.save(Saved::Continue)
            }
            (Order::Start { .. }, _, Agent::Up { .. })
            | (Order::Stop(_), Turn::Idle, Agent::Down) => {
                answer(CommandResult::Accepted)
            }
            // A closed session takes it too: `Continue` starts at it.
            (Order::Effort(effort, offered), _, _) => {
                self.set_effort(effort, &offered)
            }
            (_, _, Agent::Down) => rejected(Rejection::SessionClosed),
            (Order::Send { text, by }, Turn::Idle, _) => {
                self.save(Saved::Send(text, by))
            }
            (Order::Send { .. }, Turn::Starting(_) | Turn::Running(_), _) => {
                rejected(Rejection::TurnInProgress)
            }
            (Order::Interrupt, Turn::Idle, _) => {
                rejected(Rejection::NoTurnRunning)
            }
            (Order::Interrupt, Turn::Starting(_), _) => {
                self.turn = Turn::Starting(Held::Interrupt);
                Vec::from([
                    Effect::Trace(
                        "holding the interrupt until the turn starts",
                    ),
                    Effect::Reply(CommandResult::Accepted),
                ])
            }
            (Order::Interrupt, Turn::Running(native), _) => {
                let call = Call::Agent(Command::Interrupt(native.clone()));
                self.flow = Flow::Noting(Some(CommandResult::Accepted));
                Vec::from([Effect::Call(call)])
            }
            (Order::Reply { request, reply, by }, _, _) => {
                self.reply(request, reply, by)
            }
            (Order::Diff(_) | Order::Undo { .. }, _, _) => {
                rejected(Rejection::CheckpointUnavailable)
            }
        }
    }

    /// A model the provider no longer offers, after a credential change,
    /// gives way to its default, and the log says so.
    fn offered(&mut self, offered: &Models) -> Vec<Effect> {
        match self.choice.model.clone().filter(|model| {
            offered.default.as_ref() != Some(model)
                && offered.find(model).is_none()
        }) {
            Some(gone) => {
                let choice = choose(offered, None, self.choice.effort)
                    .unwrap_or_else(|_| Choice {
                        model: offered.default.clone(),
                        effort: None,
                    });
                let reason = format!(
                    "Model changed to {}: {} is not offered by the current credential",
                    choice.model.as_ref().map_or_else(
                        || String::from("the default model"),
                        |model| model.0.clone()
                    ),
                    gone.0
                );
                self.choice = choice.clone();
                Vec::from([Effect::Emit(EventKind::ModelChanged {
                    model: choice.model,
                    effort: choice.effort,
                    reason,
                })])
            }
            None => Vec::new(),
        }
    }

    /// The same effort again is `Accepted` and logs nothing.
    fn set_effort(&mut self, effort: Effort, offered: &Models) -> Vec<Effect> {
        match (
            takes(offered, self.choice.model.as_ref(), effort),
            self.choice.effort,
        ) {
            (Err(reason), _) => rejected(Rejection::NotOffered { reason }),
            (Ok(taken), Some(current)) if taken == current => {
                answer(CommandResult::Accepted)
            }
            (Ok(_), _) => {
                self.choice.effort = Some(effort);
                Vec::from([
                    Effect::Emit(EventKind::EffortChanged { effort }),
                    Effect::Reply(CommandResult::Accepted),
                ])
            }
        }
    }

    fn recorded(&mut self, text: String, seq: HostSeq) -> Vec<Effect> {
        self.turn_start = Some(seq);
        self.book.accept_message(seq);
        let change = changed(self.choice.effort, self.agent.effort());
        self.send(text, change)
    }

    /// Starts a turn, at the session's effort if the agent runs another.
    /// The session takes the turn's id from `TurnStarted`, which an
    /// interrupt must wait for anyway.
    fn send(&mut self, text: String, change: Option<Effort>) -> Vec<Effect> {
        self.flow = Flow::Sending(change);
        Vec::from([Effect::Call(Call::Agent(Command::Send {
            text,
            effort: change,
        }))])
    }

    /// A message the agent never got starts no turn.
    fn sent(
        &mut self,
        change: Option<Effort>,
        sent: Result<(), String>,
    ) -> Vec<Effect> {
        match sent {
            Ok(()) => {
                if let Agent::Up { effort, .. } = &mut self.agent {
                    *effort = change.or(*effort);
                }
                self.conversation = self.agent.conversation();
                self.turn = Turn::Starting(Held::Nothing);
                join(
                    self.conversation.clone().map(Effect::Conversation),
                    [Effect::Reply(CommandResult::Accepted)],
                )
            }
            Err(message) => {
                self.turn_start = None;
                Vec::from([
                    self.checkpoint_event(EventKind::TurnCompleted {
                        outcome: TurnOutcome::Failed { message },
                    }),
                    Effect::Reply(CommandResult::Accepted),
                ])
            }
        }
    }

    fn reply(
        &mut self,
        request: RequestId,
        reply: Reply,
        by: ClientName,
    ) -> Vec<Effect> {
        match self.requests.get(&request).cloned() {
            None | Some(Pending::Resolved(Resolution::Withdrawn)) => {
                rejected(Rejection::UnknownRequest)
            }
            Some(Pending::Resolved(
                Resolution::Decided { by: first, .. }
                | Resolution::Answered { by: first, .. },
            )) => rejected(Rejection::AlreadyResolved { by: first }),
            Some(Pending::Open { native, asked }) => match fits(&asked, &reply)
            {
                Err(reason) => rejected(Rejection::WrongReply { reason }),
                Ok(()) => self.resolve(request, (native, asked), reply, by),
            },
        }
    }

    fn resolve(
        &mut self,
        request: RequestId,
        (native, asked): (NativeRequest, Asked),
        reply: Reply,
        by: ClientName,
    ) -> Vec<Effect> {
        if let (
            Asked::Approval { agent, call, .. },
            Reply::Decide(Decision::Deny),
        ) = (asked, &reply)
        {
            let denied = (agent, call);
            if !self.declined.contains(&denied) {
                self.declined.push(denied);
            }
        }
        let resolution = match reply.clone() {
            Reply::Decide(decision) => Resolution::Decided {
                decision,
                by: by.clone(),
            },
            Reply::Answer(answers) => Resolution::Answered {
                answers,
                by: by.clone(),
            },
        };
        self.requests
            .insert(request, Pending::Resolved(resolution.clone()));
        self.flow = Flow::Noting(Some(CommandResult::Accepted));
        Vec::from([
            Effect::Emit(EventKind::RequestResolved {
                request,
                resolution,
            }),
            Effect::Call(Call::Agent(Command::Reply { native, reply, by })),
        ])
    }

    fn agent_step(&mut self, step: Step) -> Vec<Effect> {
        match step {
            Step::Emit(kind) => Vec::from([Effect::Emit(kind)]),
            Step::Open {
                agent,
                native,
                call,
                tool,
                reason,
                for_session,
            } => self.open_approval(
                agent,
                native,
                call,
                tool,
                reason,
                for_session,
            ),
            Step::Ask {
                agent,
                native,
                questions,
            } => {
                let asked = Asked::Question {
                    agent: agent.clone(),
                    count: questions.len(),
                };
                let request = self.open(native, asked);
                match agent {
                    Some(scope) => subagent(
                        scope,
                        SubagentEvent::QuestionAsked { request, questions },
                    ),
                    None => {
                        Vec::from([Effect::Emit(EventKind::QuestionAsked {
                            request,
                            questions,
                        })])
                    }
                }
            }
            Step::ToolFinished { call, outcome } => {
                self.tool_finished(call, outcome)
            }
            Step::Withdraw(native) => self.withdraw(&native),
            Step::Checklist(steps) => self.checklist(None, steps),
            Step::SubagentChecklist { agent, steps } => {
                self.checklist(Some(agent), steps)
            }
            Step::SubagentKind { agent, kind } => {
                subagent(agent, SubagentEvent::Kind { kind })
            }
            Step::SubagentStarted {
                agent,
                call,
                description,
                kind,
            } => {
                let mut effects = subagent(
                    agent.clone(),
                    SubagentEvent::Started { call, description },
                );
                if let Some(agent_kind) = kind {
                    effects.extend(subagent(
                        agent,
                        SubagentEvent::Kind { kind: agent_kind },
                    ));
                }
                effects
            }
            Step::SubagentActivity { agent, activity } => {
                let recorded = self.subagent_activity(&agent, activity);
                subagent(agent, SubagentEvent::Activity(recorded))
            }
            Step::SubagentEnded {
                agent,
                outcome,
                summary,
            } => join(
                self.withdraw_scope(Some(&agent)),
                subagent(agent, SubagentEvent::Ended { outcome, summary }),
            ),
            // A request still open when the turn ends can't be answered any
            // more: an agent may withdraw it only after `TurnCompleted`, and an
            // answer in between would log a decision for a command that
            // never runs. Its later withdrawal then finds nothing open.
            Step::HistoryBoundary(boundary) => self.history_boundary(boundary),
            Step::TurnEnded(outcome) => self.save(Saved::TurnEnded(outcome)),
            Step::TurnStarted(native) => self.turn_started(native),
            // Nothing the gone agent asked can be answered, and a turn it
            // ran is over; clients mark both when they see the close.
            Step::Closed(reason) => match &self.turn {
                Turn::Running(_) => self.save(Saved::Closed(reason)),
                Turn::Idle | Turn::Starting(_) => self.closed(reason),
            },
        }
    }

    fn open_approval(
        &mut self,
        agent: Option<SubagentId>,
        native: NativeRequest,
        call: ToolCallId,
        tool: ToolKind,
        reason: Option<String>,
        for_session: Option<String>,
    ) -> Vec<Effect> {
        let asked = Asked::Approval {
            agent: agent.clone(),
            call: call.clone(),
            for_session: for_session.clone(),
        };
        let request = self.open(native, asked);
        match agent {
            Some(scope) => subagent(
                scope,
                SubagentEvent::RequestOpened {
                    request,
                    call,
                    tool,
                    reason,
                    for_session,
                },
            ),
            None => Vec::from([Effect::Emit(EventKind::RequestOpened {
                request,
                call,
                tool,
                reason,
                for_session,
            })]),
        }
    }

    fn tool_finished(
        &mut self,
        call: ToolCallId,
        outcome: ToolOutcome,
    ) -> Vec<Effect> {
        let finished = self.finished(None, &call, outcome);
        Vec::from([Effect::Emit(EventKind::ToolCall {
            call,
            phase: ToolPhase::Finished { outcome: finished },
        })])
    }

    fn subagent_activity(
        &mut self,
        agent: &SubagentId,
        activity: SubagentActivity,
    ) -> SubagentActivity {
        match activity {
            SubagentActivity::ToolCall {
                call,
                phase: ToolPhase::Finished { outcome },
            } => SubagentActivity::ToolCall {
                phase: ToolPhase::Finished {
                    outcome: self.finished(Some(agent), &call, outcome),
                },
                call,
            },
            other => other,
        }
    }

    /// A call that failed after the user denied it was declined.
    fn finished(
        &mut self,
        agent: Option<&SubagentId>,
        call: &ToolCallId,
        outcome: ToolOutcome,
    ) -> ToolOutcome {
        let matches = |(owner, denied): &(Option<SubagentId>, ToolCallId)| {
            owner.as_ref() == agent && denied == call
        };
        let denied = self.declined.iter().position(matches);
        self.declined.retain(|denied| !matches(denied));
        match (outcome, denied) {
            (ToolOutcome::Failed, Some(_)) => ToolOutcome::Declined,
            (reported, _) => reported,
        }
    }

    fn history_boundary(&mut self, boundary: Option<String>) -> Vec<Effect> {
        boundary.map_or_else(Vec::new, |message| {
            let history = self.anchor(None, Some(message));
            Vec::from([self
                .checkpoint_event(EventKind::HistoryMaterialized { history })])
        })
    }

    fn checklist(
        &mut self,
        agent: Option<SubagentId>,
        snapshot: Vec<ChecklistEntry>,
    ) -> Vec<Effect> {
        match agent {
            Some(scope) => {
                let previous =
                    self.subagent_tasks.entry(scope.clone()).or_default();
                if *previous == snapshot {
                    Vec::new()
                } else {
                    previous.clone_from(&snapshot);
                    subagent(
                        scope,
                        SubagentEvent::ChecklistChanged { tasks: snapshot },
                    )
                }
            }
            None => self.retask(snapshot),
        }
    }

    fn retask(&mut self, tasks: Vec<ChecklistEntry>) -> Vec<Effect> {
        if tasks == self.tasks {
            Vec::new()
        } else {
            self.tasks.clone_from(&tasks);
            Vec::from([Effect::Emit(EventKind::ChecklistChanged { tasks })])
        }
    }

    fn turn_ended(&mut self, outcome: TurnOutcome) -> Vec<Effect> {
        self.turn = Turn::Idle;
        self.turn_start = None;
        join(
            [Effect::Emit(EventKind::TurnCompleted { outcome })],
            self.withdraw_scope(None),
        )
    }

    fn withdraw_scope(&mut self, agent: Option<&SubagentId>) -> Vec<Effect> {
        let open = self
            .requests
            .iter()
            .filter_map(|(request, pending)| match pending {
                Pending::Open { asked, .. } if asked.agent() == agent => {
                    Some(*request)
                }
                _ => None,
            })
            .collect::<Vec<RequestId>>();
        open.into_iter()
            .flat_map(|request| self.withdrawn(request))
            .collect()
    }

    fn turn_started(&mut self, native: NativeTurn) -> Vec<Effect> {
        self.turn_start = match &self.turn {
            Turn::Idle => None,
            Turn::Starting(_) | Turn::Running(_) => self.turn_start,
        };
        self.conversation = self.agent.conversation();
        let saved = self.conversation.clone().map(Effect::Conversation);
        let materialized = match (self.book.workspace(), &self.agent) {
            (
                Some(_),
                Agent::Up {
                    anchoring: Anchoring::Turns,
                    ..
                },
            ) => {
                let history = self.anchor(Some(&native), None);
                Some(self.checkpoint_event(EventKind::HistoryMaterialized {
                    history,
                }))
            }
            _ => None,
        };
        let started = self.book.workspace().is_some().then(|| {
            let history = self.anchor(Some(&native), None);
            self.checkpoint_event(EventKind::NativeTurnStarted {
                history,
                turn_start: self.turn_start,
            })
        });
        let before =
            mem::replace(&mut self.turn, Turn::Running(native.clone()));
        let held = match before {
            Turn::Starting(Held::Interrupt) => {
                self.flow = Flow::Noting(None);
                Vec::from([
                    Effect::Trace(
                        "the turn started: sending the held interrupt",
                    ),
                    Effect::Call(Call::Agent(Command::Interrupt(native))),
                ])
            }
            Turn::Starting(Held::Nothing) | Turn::Idle | Turn::Running(_) => {
                Vec::new()
            }
        };
        join(join(join(saved, materialized), started), held)
    }

    /// Removes the closed session's workspace, once what the log staged is
    /// committed; a session without one is removed at once.
    fn clean(&mut self) -> Vec<Effect> {
        match self.book.workspace().cloned() {
            Some(workspace) => {
                self.flow = Flow::Removing;
                Vec::from([
                    Effect::Barrier,
                    Effect::Call(Call::Remove(workspace)),
                ])
            }
            None => self.gone(None),
        }
    }

    fn gone(&mut self, kept: Option<Kept>) -> Vec<Effect> {
        self.removed = true;
        Vec::from([
            Effect::Emit(EventKind::SessionRemoved { kept }),
            Effect::Reply(CommandResult::Accepted),
        ])
    }

    fn closed(&mut self, reason: CloseReason) -> Vec<Effect> {
        self.agent = Agent::Down;
        self.turn = Turn::Idle;
        self.turn_start = None;
        self.declined.clear();
        for pending in self.requests.values_mut() {
            if let Pending::Open { .. } = pending {
                *pending = Pending::Resolved(Resolution::Withdrawn);
            }
        }
        Vec::from([Effect::Emit(EventKind::SessionClosed { reason })])
    }

    fn anchor(
        &self,
        native: Option<&NativeTurn>,
        message: Option<String>,
    ) -> HistoryAnchor {
        match &self.agent {
            Agent::Up {
                anchoring: Anchoring::Messages,
                conversation,
                ..
            } => HistoryAnchor::Messages {
                conversation: conversation.clone(),
                message,
            },
            Agent::Up {
                anchoring: Anchoring::Turns,
                conversation,
                ..
            } => HistoryAnchor::Turns {
                conversation: conversation.clone(),
                turn: native.map(|turn| turn.0.clone()),
            },
            Agent::Down => self
                .book
                .resume(self.conversation.is_some())
                .unwrap_or(HistoryAnchor::Fresh),
        }
    }

    fn checkpoint_event(&mut self, kind: EventKind) -> Effect {
        self.book.apply_kind(&kind);
        Effect::Emit(kind)
    }

    fn save(&mut self, then: Saved) -> Vec<Effect> {
        match self.book.plan_snapshot() {
            Some(plan) => {
                let checkpoint = plan.reserved;
                let call = Call::Snapshot {
                    workspace: plan.workspace.clone(),
                    checkpoint,
                    previous: plan.base.clone(),
                };
                self.flow = Flow::Saving(Saving { plan, then });
                Vec::from([
                    self.checkpoint_event(EventKind::CheckpointReserved {
                        checkpoint,
                    }),
                    Effect::Barrier,
                    Effect::Call(call),
                ])
            }
            None => self.saved(then, Ok(())),
        }
    }

    fn snapshotted(
        &mut self,
        saving: Saving,
        taken: Result<String, String>,
    ) -> Vec<Effect> {
        let Saving { plan, then } = saving;
        match taken {
            Ok(commit) => {
                let created =
                    self.checkpoint_event(EventKind::CheckpointCreated {
                        checkpoint: plan.checkpoint(commit),
                    });
                join([created], self.saved(then, Ok(())))
            }
            Err(reason) => self.saved(then, Err(reason)),
        }
    }

    fn saved(&mut self, then: Saved, saved: Result<(), String>) -> Vec<Effect> {
        match (then, saved) {
            (Saved::Start(offered), Ok(())) => {
                let changed = self.offered(&offered);
                let at = (
                    self.book.resume(self.conversation.is_some()),
                    self.conversation.clone(),
                );
                let directory = self.directory.clone();
                join(changed, self.start(directory, at, Launched::Ready))
            }
            (Saved::Continue, Ok(())) => answer(CommandResult::Accepted),
            (Saved::Send(text, by), Ok(())) => {
                self.flow = Flow::Recording(text.clone());
                Vec::from([Effect::Call(Call::Record(
                    EventKind::UserMessage { text, by },
                ))])
            }
            (
                Saved::Start(..) | Saved::Continue | Saved::Send(..),
                Err(reason),
            ) => rejected(Rejection::WorkspaceFailure { reason }),
            (Saved::TurnEnded(outcome), taken) => {
                join(checkpoint_failed(taken), self.turn_ended(outcome))
            }
            (Saved::Closed(reason), taken) => {
                join(checkpoint_failed(taken), self.closed(reason))
            }
        }
    }

    /// Starts an agent in `directory` at `at`, in place of the one running
    /// if it starts.
    fn start(
        &mut self,
        directory: HostPath,
        (history, conversation): At,
        then: Launched,
    ) -> Vec<Effect> {
        self.flow = Flow::Starting(then);
        Vec::from([Effect::Call(Call::Agent(Command::Start(Start {
            session: self.id,
            directory,
            model: self.choice.model.clone(),
            effort: self.choice.effort,
            history,
            conversation,
            instructions: None,
        })))])
    }

    fn started(
        &mut self,
        then: Launched,
        started: Result<Started, String>,
    ) -> Vec<Effect> {
        match started {
            Ok(Started {
                conversation,
                anchoring,
            }) => {
                self.agent = Agent::Up {
                    anchoring,
                    conversation: conversation.clone(),
                    effort: self.choice.effort,
                };
                self.launched(then, Ok(conversation))
            }
            Err(message) => self.launched(then, Err(message)),
        }
    }

    fn launched(
        &mut self,
        then: Launched,
        started: Result<String, String>,
    ) -> Vec<Effect> {
        match (then, started) {
            (Launched::Ready, Ok(conversation)) => Vec::from([
                Effect::Conversation(conversation),
                Effect::Emit(EventKind::SessionReady),
                Effect::Reply(CommandResult::Accepted),
            ]),
            (Launched::Ready, Err(message)) => Vec::from([
                Effect::Emit(EventKind::SessionClosed {
                    reason: CloseReason::FailedToStart { message },
                }),
                Effect::Reply(CommandResult::Accepted),
            ]),
            (Launched::Revert(revert), Ok(_)) => self.list(revert),
            (Launched::Reconcile(revert), Ok(_)) => {
                self.flow = Flow::Reconciling(revert);
                Vec::from([Effect::Call(Call::Agent(Command::Turns))])
            }
            (Launched::Reconcile(_), Err(reason)) => {
                verification_pending(&reason)
            }
            (
                Launched::Restart(undoing, directory, history),
                Ok(conversation),
            ) => {
                self.conversation = None;
                join(
                    [Effect::Conversation(conversation)],
                    self.restored(undoing, &directory, Ok(history)),
                )
            }
            (Launched::Complete(undoing), Ok(_)) => {
                self.finish_undo(&undoing.undo)
            }
            (
                Launched::Revert(Revert {
                    undoing, directory, ..
                })
                | Launched::Restart(undoing, directory, _),
                Err(reason),
            ) => self.restored(undoing, &directory, Err(reason)),
            (Launched::Complete(undoing), Err(reason)) => {
                self.undo_failed(undoing.undo.selected, reason)
            }
        }
    }

    fn diff(&mut self, id: CheckpointId) -> Vec<Effect> {
        match self.book.diff(id).map(Box::new).map(Call::Diff) {
            Some(call) => {
                self.flow = Flow::Diffing;
                Vec::from([Effect::Call(call)])
            }
            None => rejected(Rejection::CheckpointUnavailable),
        }
    }

    fn undo(&mut self, checkpoint: CheckpointId, fork: &str) -> Vec<Effect> {
        match self.book.plan_undo(
            checkpoint,
            fork,
            !matches!(self.turn, Turn::Idle),
        ) {
            Err(reason) => rejected(reason),
            Ok(UndoPlan::Start {
                undo,
                workspace,
                commit,
            }) => {
                self.flow = Flow::Preflighting(undo);
                Vec::from([Effect::Call(Call::Preflight { workspace, commit })])
            }
            Ok(UndoPlan::Resume(
                Recovery::Native(undo) | Recovery::Verify(undo),
            )) => self.restore_native(undo),
            Ok(UndoPlan::Resume(Recovery::Files(undo))) => {
                self.restore_files(Undoing { undo })
            }
        }
    }

    fn preflighted(
        &mut self,
        undo: Undo,
        checked: Result<(), String>,
    ) -> Vec<Effect> {
        match checked {
            Ok(()) => {
                let started = self.checkpoint_event(EventKind::UndoStarted {
                    checkpoint: undo.selected,
                    restore: undo.restored,
                    history: undo.history.clone(),
                });
                join([started, Effect::Barrier], self.restore_native(undo))
            }
            Err(reason) => rejected(Rejection::WorkspaceFailure { reason }),
        }
    }

    fn restore_native(&mut self, undo: Undo) -> Vec<Effect> {
        let history = undo.history.clone();
        self.flow = Flow::Locating(Undoing { undo }, history);
        Vec::from([Effect::Call(Call::Exists(self.directory.clone()))])
    }

    /// The agent's history is restored in the session's directory, or in
    /// the worktree when that directory is gone.
    fn located(
        &mut self,
        undoing: Undoing,
        history: HistoryAnchor,
        exists: bool,
    ) -> Vec<Effect> {
        let directory = match (exists, self.book.workspace()) {
            (false, Some(workspace)) => workspace.worktree.clone(),
            _ => self.directory.clone(),
        };
        match history {
            HistoryAnchor::Turns {
                conversation,
                turn: Some(turn),
            } => self.launch_revert(
                Revert {
                    undoing,
                    directory,
                    conversation,
                    turn,
                },
                if self.book.awaiting_revert() {
                    Launched::Reconcile
                } else {
                    Launched::Revert
                },
            ),
            HistoryAnchor::Fresh | HistoryAnchor::Fork { .. } => {
                self.agent = Agent::Down;
                self.flow = Flow::Restarting(undoing, directory, history);
                Vec::from([Effect::Call(Call::Agent(Command::Shutdown))])
            }
            _ => self.restored(
                undoing,
                &directory,
                Err(String::from("native undo boundary is unavailable")),
            ),
        }
    }

    fn launch_revert(
        &mut self,
        revert: Revert,
        then: fn(Revert) -> Launched,
    ) -> Vec<Effect> {
        let at = (
            Some(HistoryAnchor::Turns {
                conversation: revert.conversation.clone(),
                turn: None,
            }),
            None,
        );
        self.start(revert.directory.clone(), at, then(revert))
    }

    /// The agent stopped; it starts again in `directory` at `history`.
    fn restart(
        &mut self,
        undoing: Undoing,
        directory: HostPath,
        history: HistoryAnchor,
        stopped: Result<(), String>,
    ) -> Vec<Effect> {
        match stopped {
            Ok(()) => {
                let at = (Some(history.clone()), None);
                let then =
                    Launched::Restart(undoing, directory.clone(), history);
                self.start(directory, at, then)
            }
            Err(reason) => self.restored(undoing, &directory, Err(reason)),
        }
    }

    fn list(&mut self, revert: Revert) -> Vec<Effect> {
        match &self.agent {
            Agent::Up {
                anchoring: Anchoring::Turns,
                ..
            } => {
                self.flow = Flow::Listing(revert);
                Vec::from([Effect::Call(Call::Agent(Command::Turns))])
            }
            _ => self.restored(
                revert.undoing,
                &revert.directory,
                Err(String::from(
                    "the agent restored to revert turns keeps no turns",
                )),
            ),
        }
    }

    fn listed(
        &mut self,
        revert: Revert,
        listed: Result<Vec<String>, String>,
    ) -> Vec<Effect> {
        match listed {
            Ok(before) if before.contains(&revert.turn) => {
                let call = Call::Agent(Command::Revert(revert.turn.clone()));
                let dispatched =
                    self.checkpoint_event(EventKind::UndoRevertDispatched {
                        checkpoint: revert.undoing.undo.selected,
                    });
                self.flow = Flow::Reverting(revert);
                Vec::from([dispatched, Effect::Barrier, Effect::Call(call)])
            }
            Ok(_) => self.reverted(revert, Ok(())),
            Err(reason) => {
                self.restored(revert.undoing, &revert.directory, Err(reason))
            }
        }
    }

    fn reverted(
        &mut self,
        revert: Revert,
        reverted: Result<(), String>,
    ) -> Vec<Effect> {
        match reverted {
            Ok(()) => {
                self.flow = Flow::Checking(revert);
                Vec::from([Effect::Call(Call::Agent(Command::Turns))])
            }
            Err(reason) => {
                self.restored(revert.undoing, &revert.directory, Err(reason))
            }
        }
    }

    /// The reverted thread's turns must be those of the checkpoints kept.
    fn checked(
        &mut self,
        revert: Revert,
        listed: Result<Vec<String>, String>,
    ) -> Vec<Effect> {
        let desired = self.book.retained_turns(revert.undoing.undo.restored);
        let history = listed.and_then(|after| {
            (after == desired)
                .then(|| HistoryAnchor::Turns {
                    conversation: revert.conversation,
                    turn: desired.last().cloned(),
                })
                .ok_or_else(|| {
                    String::from(
                        "native history does not match the retained turn prefix",
                    )
                })
        });
        match history {
            Err(reason) if self.book.awaiting_revert() => {
                verification_pending(&reason)
            }
            other => self.restored(revert.undoing, &revert.directory, other),
        }
    }

    /// An agent restored outside the session's directory stops, so the
    /// session starts it again in its own.
    fn restored(
        &mut self,
        undoing: Undoing,
        directory: &HostPath,
        restored: Result<HistoryAnchor, String>,
    ) -> Vec<Effect> {
        match restored {
            Ok(history) if directory != &self.directory => {
                self.agent = Agent::Down;
                self.flow = Flow::Leaving(undoing, history);
                Vec::from([Effect::Call(Call::Agent(Command::Shutdown))])
            }
            other => self.history_restored(undoing, other),
        }
    }

    fn history_restored(
        &mut self,
        undoing: Undoing,
        restored: Result<HistoryAnchor, String>,
    ) -> Vec<Effect> {
        match restored {
            Err(reason) => self.undo_failed(undoing.undo.selected, reason),
            Ok(history) => {
                let restored =
                    self.checkpoint_event(EventKind::UndoHistoryRestored {
                        checkpoint: undoing.undo.selected,
                        history,
                    });
                self.flow = Flow::Retasking(undoing);
                Vec::from([restored, Effect::Call(Call::History)])
            }
        }
    }

    /// The tasks go back to how they were when the restored checkpoint was
    /// taken, and the undo's native phase is durable.
    fn retasked(&mut self, undoing: Undoing, events: &[Event]) -> Vec<Effect> {
        let tasks =
            self.retask(tasks_before(events, self.id, undoing.undo.restored));
        join(join(tasks, [Effect::Barrier]), self.restore_files(undoing))
    }

    fn restore_files(&mut self, undoing: Undoing) -> Vec<Effect> {
        match self.book.restore_point(undoing.undo.restored) {
            Some((workspace, commit)) => {
                self.flow = Flow::Restoring(undoing);
                Vec::from([Effect::Call(Call::Restore { workspace, commit })])
            }
            None => self.undo_failed(
                undoing.undo.selected,
                String::from("restore checkpoint is unavailable"),
            ),
        }
    }

    fn files_restored(
        &mut self,
        undoing: Undoing,
        restored: Result<(), String>,
    ) -> Vec<Effect> {
        match (restored, &self.agent) {
            (Err(reason), _) => self.undo_failed(undoing.undo.selected, reason),
            (Ok(()), Agent::Down) => {
                let at = (
                    self.book.resume(self.conversation.is_some()),
                    self.conversation.clone(),
                );
                let directory = self.directory.clone();
                self.start(directory, at, Launched::Complete(undoing))
            }
            (Ok(()), Agent::Up { .. }) => self.finish_undo(&undoing.undo),
        }
    }

    fn finish_undo(&mut self, undo: &Undo) -> Vec<Effect> {
        self.turn_start = None;
        Vec::from([
            self.checkpoint_event(EventKind::TurnsUndone {
                checkpoint: undo.selected,
                restored: undo.restored,
            }),
            Effect::Reply(CommandResult::Accepted),
        ])
    }

    fn undo_failed(
        &mut self,
        checkpoint: CheckpointId,
        reason: String,
    ) -> Vec<Effect> {
        self.flow = Flow::Idle;
        Vec::from([
            self.checkpoint_event(EventKind::UndoFailed {
                checkpoint,
                reason: reason.clone(),
            }),
            Effect::Reply(CommandResult::Rejected(Rejection::UndoFailed {
                reason,
            })),
        ])
    }

    /// Stores a new request open under the next id.
    fn open(&mut self, native: NativeRequest, asked: Asked) -> RequestId {
        let request = self.last_request.next();
        self.last_request = request;
        self.requests
            .insert(request, Pending::Open { native, asked });
        request
    }

    fn withdraw(&mut self, native: &NativeRequest) -> Vec<Effect> {
        self.requests
            .iter()
            .find(|(_, pending)| pending.native() == Some(native))
            .map(|(request, _)| *request)
            .map_or_else(Vec::new, |request| self.withdrawn(request))
    }

    fn withdrawn(&mut self, request: RequestId) -> Vec<Effect> {
        self.requests
            .insert(request, Pending::Resolved(Resolution::Withdrawn));
        Vec::from([Effect::Emit(EventKind::RequestResolved {
            request,
            resolution: Resolution::Withdrawn,
        })])
    }
}

fn removal_failed(reason: String) -> Vec<Effect> {
    Vec::from([
        Effect::Emit(EventKind::Error {
            message: format!("the session was not removed: {reason}"),
        }),
        Effect::Reply(CommandResult::Rejected(Rejection::WorkspaceFailure {
            reason,
        })),
    ])
}

fn verification_pending(reason: &str) -> Vec<Effect> {
    join(
        [Effect::Emit(EventKind::Error {
            message: format!(
                "could not verify the native rollback outcome: {reason}"
            ),
        })],
        rejected(Rejection::UndoInProgress),
    )
}

/// Whether `reply` answers what the request asked, and if not, why.
fn fits(asked: &Asked, reply: &Reply) -> Result<(), String> {
    match (asked, reply) {
        (
            Asked::Approval {
                for_session: None, ..
            },
            Reply::Decide(Decision::ApproveForSession),
        ) => Err(String::from(
            "the agent offers no approval for the session here",
        )),
        (Asked::Approval { .. }, Reply::Decide(_)) => Ok(()),
        (Asked::Approval { .. }, Reply::Answer(_)) => {
            Err(String::from("an approval takes a decision, not answers"))
        }
        (Asked::Question { .. }, Reply::Decide(_)) => {
            Err(String::from("a question takes answers, not a decision"))
        }
        (Asked::Question { count, .. }, Reply::Answer(answers))
            if &answers.len() == count =>
        {
            Ok(())
        }
        (Asked::Question { count, .. }, Reply::Answer(answers)) => {
            Err(format!("{} answers for {count} questions", answers.len()))
        }
    }
}

/// The effort the agent should change to before its next turn, if it runs
/// another than `wanted`.
fn changed(wanted: Option<Effort>, running: Option<Effort>) -> Option<Effort> {
    wanted.filter(|&effort| running != Some(effort))
}

/// The resolved requests. One left open is unknown, which a reply takes as
/// withdrawn.
fn replay_requests(history: &[Event]) -> BTreeMap<RequestId, Pending> {
    history
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::RequestResolved {
                request,
                resolution,
            } => Some((
                request.to_owned(),
                Pending::Resolved(resolution.clone()),
            )),
            _ => None,
        })
        .collect()
}

fn last_tasks<'events>(
    history: impl DoubleEndedIterator<Item = &'events Event>,
) -> Vec<ChecklistEntry> {
    history
        .rev()
        .find_map(|event| match &event.kind {
            EventKind::ChecklistChanged { tasks } => Some(tasks.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// The last checklist before the restored checkpoint, or none.
fn tasks_before(
    history: &[Event],
    session: SessionId,
    restored: CheckpointId,
) -> Vec<ChecklistEntry> {
    let taken = history
        .iter()
        .position(|event| {
            event.session == session
                && matches!(&event.kind, EventKind::CheckpointCreated { checkpoint } if checkpoint.id == restored)
        })
        .unwrap_or(0);
    last_tasks(
        history
            .iter()
            .take(taken)
            .filter(|event| event.session == session),
    )
}

fn checkpoint_failed(saved: Result<(), String>) -> Option<Effect> {
    saved.err().map(|message| {
        Effect::Emit(EventKind::Error {
            message: format!("checkpoint failed: {message}"),
        })
    })
}

/// An agent call that failed after the command was accepted is logged as
/// an `Error`, since the command's effects are already in the log.
fn noted(
    called: Result<(), String>,
    then: Option<CommandResult>,
) -> Vec<Effect> {
    join(
        called
            .err()
            .map(|message| Effect::Emit(EventKind::Error { message })),
        then.map(Effect::Reply),
    )
}

fn diffed(diff: Result<CheckpointDiff, String>) -> CommandResult {
    diff.map_or_else(
        |reason| {
            CommandResult::Rejected(Rejection::WorkspaceFailure { reason })
        },
        CommandResult::Diff,
    )
}

fn answer(result: CommandResult) -> Vec<Effect> {
    Vec::from([Effect::Reply(result)])
}

fn rejected(rejection: Rejection) -> Vec<Effect> {
    answer(CommandResult::Rejected(rejection))
}

fn subagent(agent: SubagentId, event: SubagentEvent) -> Vec<Effect> {
    Vec::from([Effect::Emit(EventKind::Subagent { agent, event })])
}

fn join(
    first: impl IntoIterator<Item = Effect>,
    then: impl IntoIterator<Item = Effect>,
) -> Vec<Effect> {
    first.into_iter().chain(then).collect()
}

#[cfg(test)]
mod tests;
