//! One session's actor: it owns the agent, the turn and the open requests, so
//! a slow agent call stalls only its own session.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::convert::Infallible;
use std::path::PathBuf;

use agents::claude::{
    ClaudeEvent, ClaudeOptions, ClaudeSession, PermissionMode, SettingSource, VersionPolicy,
};
use agents::codex::{ApprovalPolicy, CodexEvent, CodexThread, Sandbox, ThreadOptions};
use proto::{
    ClientName, CloseReason, CommandResult, Decision, EventKind, HostPath, Rejection, RequestId,
    Resolution, SessionId, ToolCallId, ToolOutcome, ToolPhase, TurnOutcome,
};
use tokio::sync::{mpsc, oneshot};

use crate::Stopped;
use crate::codex_hub::{CodexHub, HubError};
use crate::log::Log;
use crate::mapping::{self, NativeRequest, NativeTurn, Step};

/// How the host runs Claude. Built once in `main`.
#[derive(Debug, Clone)]
pub struct ClaudeLaunch {
    pub claude_path: PathBuf,
    /// `None` leaves the CLI's default model.
    pub model: Option<String>,
}

/// How the host runs Codex: threads on the hub's shared app-server.
#[derive(Debug, Clone)]
pub struct CodexLaunch {
    pub hub: CodexHub,
    /// `None` leaves the app-server's default model.
    pub model: Option<String>,
}

/// What a session task needs to start its agent.
#[derive(Debug, Clone)]
pub enum Launch {
    Claude(ClaudeLaunch),
    Codex(CodexLaunch),
}

/// A handle to one session's task. Clone it freely.
#[derive(Debug, Clone)]
pub struct Session {
    inbox: mpsc::Sender<SessionMsg>,
}

/// A command for one session, already routed to it.
#[derive(Debug)]
pub enum Order {
    Send {
        text: String,
        by: ClientName,
    },
    Interrupt,
    Resolve {
        request: RequestId,
        decision: Decision,
        by: ClientName,
    },
}

#[derive(Debug)]
struct SessionMsg {
    order: Order,
    reply: oneshot::Sender<CommandResult>,
}

/// The agent, once started, and where its events arrive.
enum Agent {
    Claude {
        session: ClaudeSession,
        events: mpsc::Receiver<ClaudeEvent>,
    },
    Codex {
        thread: CodexThread,
        events: mpsc::Receiver<CodexEvent>,
    },
}

/// An agent call that failed.
#[derive(Debug, thiserror::Error)]
enum AgentError {
    #[error("{0}")]
    Claude(#[from] agents::claude::Error),
    #[error("{0}")]
    Codex(#[from] agents::codex::Error),
    #[error("{0}")]
    Hub(#[from] HubError),
}

/// An interrupt that arrived before the agent began the turn. Sent then, it
/// would be lost (docs/p1/design.md §11), so it waits for `TurnStarted`.
enum Held {
    Nothing,
    Interrupt,
}

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
    Open {
        native: NativeRequest,
        call: ToolCallId,
    },
    Resolved(Resolution),
}

struct SessionActor {
    id: SessionId,
    agent: Agent,
    log: Log,
    turn: Turn,
    requests: HashMap<RequestId, Pending>,
    last_request: RequestId,
    /// Tool calls the user denied, whose failed result means `Declined`.
    declined: HashSet<ToolCallId>,
}

enum Input {
    Order(SessionMsg),
    Claude(ClaudeEvent),
    Codex(CodexEvent),
    /// The agent's event stream ended.
    AgentGone,
    /// Every handle was dropped.
    Closed,
}

impl Session {
    /// Starts the session's task, which starts the agent in `directory` and
    /// logs `SessionReady` or `SessionClosed`.
    pub fn spawn(id: SessionId, directory: HostPath, launch: Launch, log: Log) -> Session {
        let (inbox, messages) = mpsc::channel(16);
        tokio::spawn(run(id, directory, launch, log, messages));
        Session { inbox }
    }

    pub async fn order(&self, order: Order) -> Result<CommandResult, Stopped> {
        self.ask(|reply| SessionMsg { order, reply }).await
    }

    async fn ask<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<T>) -> SessionMsg,
    ) -> Result<T, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.inbox
            .send(build(reply))
            .await
            .map_err(|_| Stopped {})?;
        answer.await.map_err(|_| Stopped {})
    }
}

async fn run(
    id: SessionId,
    directory: HostPath,
    launch: Launch,
    log: Log,
    inbox: mpsc::Receiver<SessionMsg>,
) -> Result<Infallible, Stopped> {
    match start(directory, launch).await {
        Ok(agent) => {
            log.append(id, EventKind::SessionReady).await?;
            let actor = SessionActor {
                id,
                agent,
                log,
                turn: Turn::Idle,
                requests: HashMap::new(),
                last_request: RequestId(0),
                declined: HashSet::new(),
            };
            actor.run(inbox).await
        }
        Err(err) => {
            let reason = CloseReason::FailedToStart {
                message: err.to_string(),
            };
            log.append(id, EventKind::SessionClosed { reason }).await?;
            Err(Stopped {})
        }
    }
}

async fn start(directory: HostPath, launch: Launch) -> Result<Agent, AgentError> {
    match launch {
        Launch::Claude(ClaudeLaunch { claude_path, model }) => {
            let (session, events) = ClaudeSession::spawn(ClaudeOptions {
                claude_path,
                cwd: PathBuf::from(directory.0),
                model,
                permission_mode: PermissionMode::Default,
                resume: None,
                // As in `s1`: the user's own hooks and plugins stay out.
                setting_sources: Some(Vec::from([SettingSource::Project, SettingSource::Local])),
                stdout_log: None,
                versions: VersionPolicy::AllowUntested,
            })
            .await?;
            Ok(Agent::Claude { session, events })
        }
        // Paseo's preset: Codex asks before leaving the workspace.
        Launch::Codex(CodexLaunch { hub, model }) => {
            let (thread, events) = hub
                .start_thread(ThreadOptions {
                    cwd: PathBuf::from(directory.0),
                    model,
                    effort: None,
                    approval_policy: ApprovalPolicy::OnRequest,
                    sandbox: Sandbox::WorkspaceWrite,
                })
                .await?;
            Ok(Agent::Codex { thread, events })
        }
    }
}

impl Pending {
    fn native(&self) -> Option<&NativeRequest> {
        match self {
            Pending::Open { native, .. } => Some(native),
            Pending::Resolved(_) => None,
        }
    }
}

impl Agent {
    async fn next(&mut self) -> Input {
        match self {
            Agent::Claude { events, .. } => {
                events.recv().await.map_or(Input::AgentGone, Input::Claude)
            }
            Agent::Codex { events, .. } => {
                events.recv().await.map_or(Input::AgentGone, Input::Codex)
            }
        }
    }
}

impl SessionActor {
    async fn run(mut self, mut inbox: mpsc::Receiver<SessionMsg>) -> Result<Infallible, Stopped> {
        loop {
            let input = tokio::select! {
                msg = inbox.recv() => msg.map_or(Input::Closed, Input::Order),
                input = self.agent.next() => input,
            };
            self.on(input).await?;
        }
    }

    async fn on(&mut self, input: Input) -> Result<(), Stopped> {
        match input {
            Input::Order(SessionMsg { order, reply }) => {
                let result = self.order(order).await?;
                let _ = reply.send(result);
                Ok(())
            }
            Input::Claude(event) => self.apply(mapping::claude_steps(event)).await,
            Input::Codex(event) => self.apply(mapping::codex_steps(event)).await,
            Input::AgentGone => {
                let reason = CloseReason::Exited {
                    status: String::from("the agent's events ended"),
                };
                self.apply(Vec::from([Step::Closed(reason)])).await
            }
            Input::Closed => Err(Stopped {}),
        }
    }

    async fn order(&mut self, order: Order) -> Result<CommandResult, Stopped> {
        match (order, &self.turn) {
            (Order::Send { text, by }, Turn::Idle) => {
                self.emit(EventKind::UserMessage {
                    text: text.clone(),
                    by,
                })
                .await?;
                let sent = self.send(text).await;
                // A message the agent never got starts no turn.
                match sent {
                    Ok(()) => self.turn = Turn::Starting(Held::Nothing),
                    Err(err) => {
                        let outcome = TurnOutcome::Failed {
                            message: err.to_string(),
                        };
                        self.emit(EventKind::TurnCompleted { outcome }).await?;
                    }
                }
                Ok(CommandResult::Accepted)
            }
            (Order::Send { .. }, Turn::Starting(_) | Turn::Running(_)) => {
                Ok(CommandResult::Rejected(Rejection::TurnInProgress))
            }
            (Order::Interrupt, Turn::Idle) => Ok(CommandResult::Rejected(Rejection::NoTurnRunning)),
            (Order::Interrupt, Turn::Starting(_)) => {
                tracing::info!(
                    session = self.id.0,
                    "holding the interrupt until the turn starts"
                );
                self.turn = Turn::Starting(Held::Interrupt);
                Ok(CommandResult::Accepted)
            }
            (Order::Interrupt, Turn::Running(native)) => {
                let sent = self.interrupt(native).await;
                self.note(sent).await?;
                Ok(CommandResult::Accepted)
            }
            (
                Order::Resolve {
                    request,
                    decision,
                    by,
                },
                _,
            ) => self.resolve(request, decision, by).await,
        }
    }

    async fn resolve(
        &mut self,
        request: RequestId,
        decision: Decision,
        by: ClientName,
    ) -> Result<CommandResult, Stopped> {
        match self.requests.get(&request).cloned() {
            None | Some(Pending::Resolved(Resolution::Withdrawn)) => {
                Ok(CommandResult::Rejected(Rejection::UnknownRequest))
            }
            Some(Pending::Resolved(Resolution::Decided { by: first, .. })) => {
                Ok(CommandResult::Rejected(Rejection::AlreadyResolved {
                    by: first,
                }))
            }
            Some(Pending::Open { native, call }) => {
                if let Decision::Deny = decision {
                    self.declined.insert(call);
                }
                let resolution = Resolution::Decided {
                    decision,
                    by: by.clone(),
                };
                self.requests
                    .insert(request, Pending::Resolved(resolution.clone()));
                self.emit(EventKind::RequestResolved {
                    request,
                    resolution,
                })
                .await?;
                let sent = self.answer(native, decision, &by).await;
                self.note(sent).await?;
                Ok(CommandResult::Accepted)
            }
        }
    }

    /// Codex returns the turn's id here too; the session takes it from
    /// `TurnStarted`, which an interrupt must wait for anyway.
    async fn send(&self, text: String) -> Result<(), AgentError> {
        match &self.agent {
            Agent::Claude { session, .. } => Ok(session.send_user(text).await?),
            Agent::Codex { thread, .. } => {
                let _ = thread.send_turn(text).await?;
                Ok(())
            }
        }
    }

    async fn interrupt(&self, turn: &NativeTurn) -> Result<(), AgentError> {
        match (&self.agent, turn) {
            (Agent::Claude { session, .. }, NativeTurn::Claude) => Ok(session.interrupt().await?),
            (Agent::Codex { thread, .. }, NativeTurn::Codex(turn)) => {
                Ok(thread.interrupt(turn.clone()).await?)
            }
            (Agent::Claude { .. }, NativeTurn::Codex(_))
            | (Agent::Codex { .. }, NativeTurn::Claude) => {
                tracing::error!(session = self.id.0, "a turn of the other provider");
                Ok(())
            }
        }
    }

    async fn answer(
        &self,
        native: NativeRequest,
        decision: Decision,
        by: &ClientName,
    ) -> Result<(), AgentError> {
        match (&self.agent, native) {
            (Agent::Claude { session, .. }, NativeRequest::Claude(id)) => Ok(session
                .respond(id, mapping::claude_answer(decision, by))
                .await?),
            (Agent::Codex { thread, .. }, NativeRequest::Codex(id)) => {
                Ok(thread.respond(id, mapping::codex_answer(decision)).await?)
            }
            (Agent::Claude { .. }, NativeRequest::Codex(_))
            | (Agent::Codex { .. }, NativeRequest::Claude(_)) => {
                tracing::error!(session = self.id.0, "a request of the other provider");
                Ok(())
            }
        }
    }

    /// An agent call that failed after the command was accepted is logged as
    /// an `Error`, since the command's effects are already in the log.
    async fn note(&self, sent: Result<(), AgentError>) -> Result<(), Stopped> {
        match sent {
            Ok(()) => Ok(()),
            Err(err) => {
                self.emit(EventKind::Error {
                    message: err.to_string(),
                })
                .await
            }
        }
    }

    /// Applies the steps in order, until one ends the session.
    async fn apply(&mut self, steps: Vec<Step>) -> Result<(), Stopped> {
        for step in steps {
            self.step(step).await?;
        }
        Ok(())
    }

    async fn step(&mut self, step: Step) -> Result<(), Stopped> {
        match step {
            Step::Emit(kind) => self.emit(kind).await,
            Step::Open { native, call, tool } => {
                let request = self.last_request.next();
                self.last_request = request;
                self.requests.insert(
                    request,
                    Pending::Open {
                        native,
                        call: call.clone(),
                    },
                );
                self.emit(EventKind::RequestOpened {
                    request,
                    call,
                    tool,
                })
                .await
            }
            Step::ToolFinished { call, outcome } => {
                let outcome = match (outcome, self.declined.take(&call)) {
                    (ToolOutcome::Failed, Some(_)) => ToolOutcome::Declined,
                    (outcome, _) => outcome,
                };
                self.emit(EventKind::ToolCall {
                    call,
                    phase: ToolPhase::Finished { outcome },
                })
                .await
            }
            Step::Withdraw(native) => self.withdraw(native).await,
            // A request still open when the turn ends can't be answered any
            // more: Codex withdraws it only after `TurnCompleted`, and an
            // answer in between would log a decision for a command that
            // never runs. Its later withdrawal then finds nothing open.
            Step::TurnEnded(outcome) => {
                self.turn = Turn::Idle;
                self.emit(EventKind::TurnCompleted { outcome }).await?;
                let open = self
                    .requests
                    .iter()
                    .filter(|(_, pending)| matches!(pending, Pending::Open { .. }))
                    .map(|(request, _)| request)
                    .copied()
                    .collect::<BTreeSet<RequestId>>();
                for request in open {
                    self.withdrawn(request).await?;
                }
                Ok(())
            }
            Step::TurnStarted(native) => {
                let before = std::mem::replace(&mut self.turn, Turn::Running(native.clone()));
                match before {
                    Turn::Starting(Held::Interrupt) => {
                        tracing::info!(
                            session = self.id.0,
                            "the turn started: sending the held interrupt"
                        );
                        let sent = self.interrupt(&native).await;
                        self.note(sent).await
                    }
                    Turn::Starting(Held::Nothing) | Turn::Idle | Turn::Running(_) => Ok(()),
                }
            }
            Step::Closed(reason) => {
                self.emit(EventKind::SessionClosed { reason }).await?;
                Err(Stopped {})
            }
        }
    }

    async fn withdraw(&mut self, native: NativeRequest) -> Result<(), Stopped> {
        let found = self
            .requests
            .iter()
            .find(|(_, pending)| pending.native() == Some(&native))
            .map(|(request, _)| request)
            .copied();
        match found {
            Some(request) => self.withdrawn(request).await,
            None => Ok(()),
        }
    }

    async fn withdrawn(&mut self, request: RequestId) -> Result<(), Stopped> {
        self.requests
            .insert(request, Pending::Resolved(Resolution::Withdrawn));
        self.emit(EventKind::RequestResolved {
            request,
            resolution: Resolution::Withdrawn,
        })
        .await
    }

    async fn emit(&self, kind: EventKind) -> Result<(), Stopped> {
        self.log.append(self.id, kind).await
    }
}
