//! One session's decisions, as a state machine: its turn, its open
//! requests, its effort, its checklist and its commit snapshots.
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

use crate::log::{Event, HostSeq};
use crate::prelude::*;
use crate::snapshots::Book;
pub use crate::snapshots::{Compared, SnapshotPlan};
use crate::workspace::{Kept, Snapshot, SnapshotDiff, Workspace};
use agents::Output;
use agents::runtime::{Command, Outcome, Start, Started};
use agents::{
    ChecklistEntry, ClientName, CloseReason, Decision, Effort, ModelId, Models,
    SubagentActivity, SubagentId, ToolCallId, ToolKind, ToolOutcome, ToolPhase,
    TurnOutcome,
};
use agents::{NativeRequest, NativeTurn, Reply, Step};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;

mod record;
pub use record::{
    EventKind, RequestId, Resolution, SessionCommand, SessionDirectory,
    SessionId, SessionRefusal, SessionResult, StartAt, SubagentEvent,
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
    Diff(HostSeq),
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
    Diffed(Result<SnapshotDiff, String>),
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
    Reply(SessionResult),
    Call(Call),
}

/// An effect whose outcome is the next [`Input`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// Append the event to the log and answer its seq.
    Record(EventKind),
    Snapshot(Workspace),
    Diff(Box<Compared>),
    /// Remove the workspace by [`crate::workspace::Plan::remove`], keeping its
    /// branch.
    Remove(Workspace),
    /// Send the agent this command; its outcome is [`Input::Agent`].
    Agent(Command),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Agent {
    Down,
    Up {
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum DeferredInterrupt {
    Nothing,
    Interrupt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Turn {
    Idle,
    Starting(DeferredInterrupt),
    Running(NativeTurn),
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
enum Asked {
    Approval {
        agent: Option<SubagentId>,
        call: ToolCallId,
        for_session: Option<String>,
    },
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

enum Flow {
    Idle,
    Saving(Saving),
    Capturing(Snapshot, Saved),
    Recording(String),
    Sending(Option<Effort>),
    Noting(Option<SessionResult>),
    Diffing,
    Starting,
    Stopping,
    Removing,
    Halting(String),
}

struct Saving {
    plan: SnapshotPlan,
    then: Saved,
}

enum Saved {
    Start(Models),
    Continue,
    Send(String, ClientName),
    TurnEnded(TurnOutcome),
    Closed(CloseReason),
}

pub struct Session {
    id: SessionId,
    directory: SessionDirectory,
    choice: Choice,
    agent: Agent,
    conversation: Option<String>,
    turn: Turn,
    requests: BTreeMap<RequestId, Pending>,
    last_request: RequestId,
    book: Book,
    turn_start: Option<HostSeq>,
    declined: Vec<(Option<SubagentId>, ToolCallId)>,
    checklist: Vec<ChecklistEntry>,
    subagent_checklists: BTreeMap<SubagentId, Vec<ChecklistEntry>>,
    removed: bool,
    flow: Flow,
}

impl Session {
    /// The session `history` logged, with no agent running. A request
    /// still open in it was the gone agent's, so it counts as withdrawn.
    #[must_use]
    pub fn restore(
        id: SessionId,
        directory: SessionDirectory,
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
            checklist: last_checklist(history.iter()),
            subagent_checklists: history
                .iter()
                .filter_map(|entry| match &entry.kind {
                    EventKind::Subagent {
                        agent,
                        event: SubagentEvent::ChecklistChanged { checklist },
                    } => Some((agent.clone(), checklist.clone())),
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
            (Flow::Starting, Input::Agent(Outcome::Started(started))) => {
                self.started(started)
            }
            (Flow::Capturing(snapshot, then), Input::Recorded(seq)) => {
                self.book.record(seq, snapshot);
                self.saved(then, Ok(()))
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
                [Effect::Reply(SessionResult::Accepted)],
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
                refuse(SessionRefusal::InstanceUnavailable { reason }),
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
            return refuse(SessionRefusal::SessionRemoved);
        }
        match order {
            Order::Diff(snapshot) => self.diff(snapshot),
            other => self.normal_order(other),
        }
    }

    fn normal_order(&mut self, order: Order) -> Vec<Effect> {
        match (order, &self.turn, &self.agent) {
            (
                Order::Remove | Order::Stop(_),
                Turn::Starting(_) | Turn::Running(_),
                _,
            ) => refuse(SessionRefusal::TurnInProgress),
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
                answer(SessionResult::Accepted)
            }
            (Order::Effort(effort, offered), _, _) => {
                self.set_effort(effort, &offered)
            }
            (_, _, Agent::Down) => refuse(SessionRefusal::SessionClosed),
            (Order::Send { text, by }, Turn::Idle, _) => {
                self.save(Saved::Send(text, by))
            }
            (Order::Send { .. }, Turn::Starting(_) | Turn::Running(_), _) => {
                refuse(SessionRefusal::TurnInProgress)
            }
            (Order::Interrupt, Turn::Idle, _) => {
                refuse(SessionRefusal::NoTurnRunning)
            }
            (Order::Interrupt, Turn::Starting(_), _) => {
                self.turn = Turn::Starting(DeferredInterrupt::Interrupt);
                Vec::from([
                    Effect::Trace(
                        "holding the interrupt until the turn starts",
                    ),
                    Effect::Reply(SessionResult::Accepted),
                ])
            }
            (Order::Interrupt, Turn::Running(native), _) => {
                let call = Call::Agent(Command::Interrupt(native.clone()));
                self.flow = Flow::Noting(Some(SessionResult::Accepted));
                Vec::from([Effect::Call(call)])
            }
            (Order::Reply { request, reply, by }, _, _) => {
                self.reply(request, reply, by)
            }
            (Order::Diff(_), _, _) => {
                refuse(SessionRefusal::SnapshotUnavailable)
            }
        }
    }

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

    fn set_effort(&mut self, effort: Effort, offered: &Models) -> Vec<Effect> {
        match (
            takes(offered, self.choice.model.as_ref(), effort),
            self.choice.effort,
        ) {
            (Err(reason), _) => refuse(SessionRefusal::NotOffered { reason }),
            (Ok(taken), Some(current)) if taken == current => {
                answer(SessionResult::Accepted)
            }
            (Ok(_), _) => {
                self.choice.effort = Some(effort);
                Vec::from([
                    Effect::Emit(EventKind::EffortChanged { effort }),
                    Effect::Reply(SessionResult::Accepted),
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

    fn send(&mut self, text: String, change: Option<Effort>) -> Vec<Effect> {
        self.flow = Flow::Sending(change);
        Vec::from([Effect::Call(Call::Agent(Command::Send {
            text,
            effort: change,
        }))])
    }

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
                self.turn = Turn::Starting(DeferredInterrupt::Nothing);
                join(
                    self.conversation.clone().map(Effect::Conversation),
                    [Effect::Reply(SessionResult::Accepted)],
                )
            }
            Err(message) => {
                self.turn_start = None;
                Vec::from([
                    self.snapshot_event(EventKind::TurnCompleted {
                        outcome: TurnOutcome::Failed { message },
                    }),
                    Effect::Reply(SessionResult::Accepted),
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
                refuse(SessionRefusal::UnknownRequest)
            }
            Some(Pending::Resolved(
                Resolution::Decided { by: first, .. }
                | Resolution::Answered { by: first, .. },
            )) => refuse(SessionRefusal::AlreadyResolved { by: first }),
            Some(Pending::Open { native, asked }) => match fits(&asked, &reply)
            {
                Err(reason) => refuse(SessionRefusal::WrongReply { reason }),
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
        self.flow = Flow::Noting(Some(SessionResult::Accepted));
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
            Step::Emit(output) => {
                Vec::from([Effect::Emit(EventKind::Agent(output))])
            }
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
            Step::TurnEnded(outcome) => self.save(Saved::TurnEnded(outcome)),
            Step::TurnStarted(native) => self.turn_started(native),
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
        Vec::from([Effect::Emit(EventKind::Agent(Output::ToolCall {
            call,
            phase: ToolPhase::Finished { outcome: finished },
        }))])
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

    fn checklist(
        &mut self,
        agent: Option<SubagentId>,
        snapshot: Vec<ChecklistEntry>,
    ) -> Vec<Effect> {
        match agent {
            Some(scope) => {
                let previous =
                    self.subagent_checklists.entry(scope.clone()).or_default();
                if *previous == snapshot {
                    Vec::new()
                } else {
                    previous.clone_from(&snapshot);
                    subagent(
                        scope,
                        SubagentEvent::ChecklistChanged {
                            checklist: snapshot,
                        },
                    )
                }
            }
            None => self.replace_checklist(snapshot),
        }
    }

    fn replace_checklist(
        &mut self,
        checklist: Vec<ChecklistEntry>,
    ) -> Vec<Effect> {
        if checklist == self.checklist {
            Vec::new()
        } else {
            self.checklist.clone_from(&checklist);
            Vec::from([Effect::Emit(EventKind::ChecklistChanged { checklist })])
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
        let started = self.book.workspace().is_some().then(|| {
            self.snapshot_event(EventKind::NativeTurnStarted {
                turn_start: self.turn_start,
            })
        });
        let before =
            mem::replace(&mut self.turn, Turn::Running(native.clone()));
        let held = match before {
            Turn::Starting(DeferredInterrupt::Interrupt) => {
                self.flow = Flow::Noting(None);
                Vec::from([
                    Effect::Trace(
                        "the turn started: sending the held interrupt",
                    ),
                    Effect::Call(Call::Agent(Command::Interrupt(native))),
                ])
            }
            Turn::Starting(DeferredInterrupt::Nothing)
            | Turn::Idle
            | Turn::Running(_) => Vec::new(),
        };
        join(join(saved, started), held)
    }

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
            Effect::Reply(SessionResult::Accepted),
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

    fn snapshot_event(&mut self, kind: EventKind) -> Effect {
        self.book.apply_kind(&kind);
        Effect::Emit(kind)
    }

    fn save(&mut self, then: Saved) -> Vec<Effect> {
        match self.book.plan_snapshot() {
            Some(plan) => {
                let call = Call::Snapshot(plan.workspace.clone());
                self.flow = Flow::Saving(Saving { plan, then });
                Vec::from([Effect::Barrier, Effect::Call(call)])
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
                let snapshot = plan.snapshot(commit);
                self.flow = Flow::Capturing(snapshot.clone(), then);
                Vec::from([Effect::Call(Call::Record(
                    EventKind::SnapshotRecorded { snapshot },
                ))])
            }
            Err(reason) => self.saved(then, Err(reason)),
        }
    }

    fn saved(&mut self, then: Saved, saved: Result<(), String>) -> Vec<Effect> {
        match (then, saved) {
            (Saved::Start(offered), Ok(())) => {
                let changed = self.offered(&offered);
                join(changed, self.start())
            }
            (Saved::Continue, Ok(())) => answer(SessionResult::Accepted),
            (Saved::Send(text, by), Ok(())) => {
                self.flow = Flow::Recording(text.clone());
                Vec::from([Effect::Call(Call::Record(
                    EventKind::UserMessage { text, by },
                ))])
            }
            (
                Saved::Start(..) | Saved::Continue | Saved::Send(..),
                Err(reason),
            ) => refuse(SessionRefusal::WorkspaceFailure { reason }),
            (Saved::TurnEnded(outcome), taken) => {
                join(snapshot_failed(taken), self.turn_ended(outcome))
            }
            (Saved::Closed(reason), taken) => {
                join(snapshot_failed(taken), self.closed(reason))
            }
        }
    }

    fn start(&mut self) -> Vec<Effect> {
        self.flow = Flow::Starting;
        Vec::from([Effect::Call(Call::Agent(Command::Start(Start {
            session: agents::runtime::Session(self.id.0),
            directory: self.directory.clone().into(),
            model: self.choice.model.clone(),
            effort: self.choice.effort,
            conversation: self.conversation.clone(),
            instructions: None,
        })))])
    }

    fn started(&mut self, started: Result<Started, String>) -> Vec<Effect> {
        match started {
            Ok(Started { conversation }) => {
                self.agent = Agent::Up {
                    conversation: conversation.clone(),
                    effort: self.choice.effort,
                };
                Vec::from([
                    Effect::Conversation(conversation),
                    Effect::Emit(EventKind::SessionReady),
                    Effect::Reply(SessionResult::Accepted),
                ])
            }
            Err(message) => Vec::from([
                Effect::Emit(EventKind::SessionClosed {
                    reason: CloseReason::FailedToStart { message },
                }),
                Effect::Reply(SessionResult::Accepted),
            ]),
        }
    }

    fn diff(&mut self, id: HostSeq) -> Vec<Effect> {
        match self.book.diff(id).map(Box::new).map(Call::Diff) {
            Some(call) => {
                self.flow = Flow::Diffing;
                Vec::from([Effect::Call(call)])
            }
            None => refuse(SessionRefusal::SnapshotUnavailable),
        }
    }

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
        Effect::Reply(SessionResult::Refused(
            SessionRefusal::WorkspaceFailure { reason },
        )),
    ])
}

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

fn changed(wanted: Option<Effort>, running: Option<Effort>) -> Option<Effort> {
    wanted.filter(|&effort| running != Some(effort))
}

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

fn last_checklist<'events>(
    history: impl DoubleEndedIterator<Item = &'events Event>,
) -> Vec<ChecklistEntry> {
    history
        .rev()
        .find_map(|event| match &event.kind {
            EventKind::ChecklistChanged { checklist } => {
                Some(checklist.clone())
            }
            _ => None,
        })
        .unwrap_or_default()
}

fn snapshot_failed(saved: Result<(), String>) -> Option<Effect> {
    saved.err().map(|message| {
        Effect::Emit(EventKind::Error {
            message: format!("snapshot failed: {message}"),
        })
    })
}

fn noted(
    called: Result<(), String>,
    then: Option<SessionResult>,
) -> Vec<Effect> {
    join(
        called
            .err()
            .map(|message| Effect::Emit(EventKind::Error { message })),
        then.map(Effect::Reply),
    )
}

fn diffed(diff: Result<SnapshotDiff, String>) -> SessionResult {
    diff.map_or_else(
        |reason| {
            SessionResult::Refused(SessionRefusal::WorkspaceFailure { reason })
        },
        SessionResult::Diff,
    )
}

fn answer(result: SessionResult) -> Vec<Effect> {
    Vec::from([Effect::Reply(result)])
}

fn refuse(refusal: SessionRefusal) -> Vec<Effect> {
    answer(SessionResult::Refused(refusal))
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
