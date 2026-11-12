//! One session's actor: it owns the agent, the turn and the open requests, so
//! a slow agent call stalls only its own session.
//!
//! The actor outlives its agent. When the agent's process exits, the
//! session is closed and keeps its conversation's id, and `Continue` starts
//! the agent again on it with the provider's resume.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::convert::Infallible;
use std::path::{Path, PathBuf};

use agents::Home;
use agents::claude::{
    ClaudeEvent, ClaudeOptions, ClaudeSession, PermissionMode, PermissionRequestId, SettingSource,
    VersionPolicy,
};
use agents::codex::{
    ApprovalId, ApprovalPolicy, CodexEvent, CodexThread, Sandbox, ThreadId, ThreadOptions, TurnId,
};
use proto::{
    Answer, ClaudeCredential, ClientName, CloseReason, CommandResult, Decision, Effort, EventKind,
    HostPath, ModelId, Models, Rejection, RequestId, Resolution, SessionId, ToolCallId,
    ToolOutcome, ToolPhase, TurnOutcome,
};
use tokio::sync::{mpsc, oneshot};

use crate::Stopped;
use crate::codex_hub::{CodexHub, HubError, Routed};
use crate::log::Log;
use crate::mapping::{self, NativeRequest, NativeTurn, Step};

/// How the host runs Claude. Built once in `main`.
#[derive(Debug, Clone)]
pub struct ClaudeLaunch {
    pub claude_path: PathBuf,
    pub home: Home,
    /// The stored credential, which the registry puts in when it starts a
    /// session.
    pub credential: Option<ClaudeCredential>,
}

/// How the host runs Codex: threads on the hub's shared app-server.
#[derive(Debug, Clone)]
pub struct CodexLaunch {
    pub hub: CodexHub,
}

/// The model and effort a session runs with, chosen when it starts. `None`
/// leaves the CLI's default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub model: Option<ModelId>,
    pub effort: Option<Effort>,
}

/// The model and effort a session asked for, checked against what the
/// provider offers. No model is the default. An effort must be one the
/// model lists, unless the host knows nothing of the model (a default from
/// `--claude-model` or a Codex credential that the CLI didn't list).
pub fn choose(
    models: &Models,
    model: Option<ModelId>,
    effort: Option<Effort>,
) -> Result<Choice, String> {
    let model = match model {
        Some(model) => models
            .find(&model)
            .map(|choice| Some(choice.id.clone()))
            .ok_or_else(|| format!("this host doesn't offer the model {}", model.0)),
        None => Ok(models.default.clone()),
    }?;
    let listed = model.as_ref().and_then(|model| models.find(model));
    match (effort, listed) {
        (Some(effort), Some(choice)) => choice
            .efforts
            .iter()
            .find(|offered| offered == &&effort)
            .map(|_| Choice {
                model: model.clone(),
                effort: Some(effort),
            })
            .ok_or_else(|| format!("{} doesn't take the effort {}", choice.name, effort.wire())),
        (effort, _) => Ok(Choice { model, effort }),
    }
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
    Reply {
        request: RequestId,
        reply: Reply,
        by: ClientName,
    },
    /// Starts the agent with this launch, resuming the conversation if the
    /// session has one: when the session starts, and on `Continue`. The
    /// models are what the provider offers now.
    Start(Launch, Models),
}

/// A reply to an open request: a decision for an approval, answers for a
/// question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Decide(Decision),
    Answer(Vec<Answer>),
}

#[derive(Debug)]
struct SessionMsg {
    order: Order,
    reply: oneshot::Sender<CommandResult>,
}

/// The agent, and where its events arrive.
enum Agent {
    Claude {
        session: ClaudeSession,
        events: mpsc::Receiver<ClaudeEvent>,
    },
    Codex {
        thread: CodexThread,
        events: mpsc::Receiver<Routed>,
    },
    /// No agent runs: it exited, the host stopped it, or it never started.
    Down,
}

/// A provider's conversation by its id: Claude's session, Codex's thread.
/// Read like [`NativeRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Conversation(String);

/// An agent call that failed.
#[derive(Debug, thiserror::Error)]
enum AgentError {
    #[error("{0}")]
    Claude(#[from] agents::claude::Error),
    #[error("{0}")]
    Codex(#[from] agents::codex::Error),
    #[error("{0}")]
    Hub(#[from] HubError),
    #[error("the agent is not running")]
    Down,
}

/// An interrupt that arrived before the agent began the turn. Sent then, it
/// would be lost (docs/architecture.md §5.1), so it waits for `TurnStarted`.
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
    Open { native: NativeRequest, asked: Asked },
    Resolved(Resolution),
}

/// What an open request waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Asked {
    /// A decision on tool call `call`. `for_session` is what approving for
    /// the session allows, if the agent offers it.
    Approval {
        call: ToolCallId,
        for_session: Option<String>,
    },
    /// One answer for each of `count` questions.
    Question { count: usize },
}

struct SessionActor {
    id: SessionId,
    directory: HostPath,
    choice: Choice,
    agent: Agent,
    /// The conversation `Continue` resumes, once a turn has started in it.
    /// Before that the CLI has written no history to resume.
    conversation: Option<Conversation>,
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
    /// The host stopped the agent, for this reason.
    Stopped(String),
    /// The agent's event stream ended.
    AgentGone,
    /// Every handle was dropped.
    Closed,
}

impl Session {
    /// Starts the session's task, which starts the agent in `directory` and
    /// logs `SessionReady` or `SessionClosed`.
    pub fn spawn(
        id: SessionId,
        directory: HostPath,
        choice: Choice,
        launch: (Launch, Models),
        log: Log,
    ) -> Session {
        let (inbox, messages) = mpsc::channel(16);
        let actor = SessionActor {
            id,
            directory,
            choice,
            agent: Agent::Down,
            conversation: None,
            log,
            turn: Turn::Idle,
            requests: HashMap::new(),
            last_request: RequestId(0),
            declined: HashSet::new(),
        };
        // The first order starts the agent, so the actor's loop does it all.
        let (reply, _) = oneshot::channel();
        let (launch, offered) = launch;
        let _ = inbox.try_send(SessionMsg {
            order: Order::Start(launch, offered),
            reply,
        });
        tokio::spawn(actor.run(messages));
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
        (Asked::Question { count }, Reply::Answer(answers)) if &answers.len() == count => Ok(()),
        (Asked::Question { count }, Reply::Answer(answers)) => {
            Err(format!("{} answers for {count} questions", answers.len()))
        }
    }
}

async fn start(
    directory: &HostPath,
    choice: &Choice,
    conversation: Option<Conversation>,
    launch: Launch,
) -> Result<Agent, AgentError> {
    match launch {
        Launch::Claude(ClaudeLaunch {
            claude_path,
            home,
            credential,
        }) => {
            let (session, events) = ClaudeSession::spawn(ClaudeOptions {
                claude_path,
                home,
                credential,
                cwd: PathBuf::from(&directory.0),
                model: choice.model.clone().map(|model| model.0),
                effort: choice.effort,
                permission_mode: PermissionMode::Default,
                resume: conversation.map(|conversation| agents::claude::SessionId(conversation.0)),
                // The user level is the host's private home (§6.2).
                setting_sources: Some(Vec::from([
                    SettingSource::User,
                    SettingSource::Project,
                    SettingSource::Local,
                ])),
                stdout_log: None,
                versions: VersionPolicy::AllowUntested,
            })
            .await?;
            Ok(Agent::Claude { session, events })
        }
        // Paseo's preset: Codex asks before leaving the workspace.
        Launch::Codex(CodexLaunch { hub }) => {
            let (thread, events) = hub
                .start_thread(
                    ThreadOptions {
                        cwd: PathBuf::from(&directory.0),
                        model: choice.model.clone().map(|model| model.0),
                        effort: choice.effort,
                        approval_policy: ApprovalPolicy::OnRequest,
                        sandbox: Sandbox::WorkspaceWrite,
                    },
                    conversation.map(|conversation| ThreadId(conversation.0)),
                )
                .await?;
            Ok(Agent::Codex { thread, events })
        }
    }
}

/// What `launch` offers with the credential in it. Claude lists its models
/// as a session would see them, started in a directory of its own under
/// `data_dir`; Codex asks the hub's app-server.
pub async fn list_models(launch: Launch, data_dir: &Path) -> Result<Models, String> {
    match launch {
        Launch::Claude(claude) => {
            let probe = data_dir.join("probe");
            std::fs::create_dir_all(&probe).map_err(|err| err.to_string())?;
            claude_models(claude, probe).await
        }
        Launch::Codex(CodexLaunch { hub }) => hub.models().await.map_err(|err| err.to_string()),
    }
}

/// What a Claude session offers, from its `initialize` response: the CLI
/// is started in `directory` and stopped again.
async fn claude_models(launch: ClaudeLaunch, directory: PathBuf) -> Result<Models, String> {
    let (session, _events) = ClaudeSession::spawn(ClaudeOptions {
        claude_path: launch.claude_path,
        home: launch.home,
        credential: launch.credential,
        cwd: directory,
        model: None,
        effort: None,
        permission_mode: PermissionMode::Default,
        resume: None,
        setting_sources: None,
        stdout_log: None,
        versions: VersionPolicy::AllowUntested,
    })
    .await
    .map_err(|err| err.to_string())?;
    let models = session.models().clone();
    let _ = session.shutdown().await;
    Ok(models)
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
                events
                    .recv()
                    .await
                    .map_or(Input::AgentGone, |routed| match routed {
                        Routed::Codex(event) => Input::Codex(event),
                        Routed::Stopped { reason } => Input::Stopped(reason),
                    })
            }
            // With no agent, the session only takes orders.
            Agent::Down => std::future::pending().await,
        }
    }

    fn conversation(&self) -> Option<Conversation> {
        match self {
            Agent::Claude { session, .. } => Some(Conversation(session.id().0.clone())),
            Agent::Codex { thread, .. } => Some(Conversation(thread.id.0.clone())),
            Agent::Down => None,
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
            Input::Stopped(reason) => {
                let reason = CloseReason::Stopped { reason };
                self.apply(Vec::from([Step::Closed(reason)])).await
            }
            Input::AgentGone => {
                let reason = CloseReason::Exited {
                    status: String::from("the agent's events ended"),
                };
                self.apply(Vec::from([Step::Closed(reason)])).await
            }
            Input::Closed => Err(Stopped {}),
        }
    }

    /// Starts the agent, resuming the conversation if it has one, and logs
    /// whether it runs.
    async fn start(&mut self, launch: Launch) -> Result<(), Stopped> {
        let resume = self.conversation.clone();
        match start(&self.directory, &self.choice, resume, launch).await {
            Ok(agent) => {
                self.agent = agent;
                self.emit(EventKind::SessionReady).await
            }
            Err(err) => {
                let reason = CloseReason::FailedToStart {
                    message: err.to_string(),
                };
                self.emit(EventKind::SessionClosed { reason }).await
            }
        }
    }

    /// A model the provider no longer offers, after a credential change,
    /// gives way to its default, and the log says so.
    async fn offered(&mut self, offered: Models) -> Result<(), Stopped> {
        match self.choice.model.clone().filter(|model| {
            offered.default.as_ref() != Some(model) && offered.find(model).is_none()
        }) {
            Some(gone) => {
                let choice = choose(&offered, None, self.choice.effort).unwrap_or(Choice {
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
                self.emit(EventKind::ModelChanged {
                    model: choice.model,
                    effort: choice.effort,
                    reason,
                })
                .await
            }
            None => Ok(()),
        }
    }

    async fn order(&mut self, order: Order) -> Result<CommandResult, Stopped> {
        match (order, &self.turn, &self.agent) {
            (Order::Start(launch, offered), _, Agent::Down) => {
                self.offered(offered).await?;
                self.start(launch).await?;
                Ok(CommandResult::Accepted)
            }
            // Continue is idempotent: the agent already runs.
            (Order::Start(_, _), _, _) => Ok(CommandResult::Accepted),
            (_, _, Agent::Down) => Ok(CommandResult::Rejected(Rejection::SessionClosed)),
            (Order::Send { text, by }, Turn::Idle, _) => {
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
            (Order::Send { .. }, Turn::Starting(_) | Turn::Running(_), _) => {
                Ok(CommandResult::Rejected(Rejection::TurnInProgress))
            }
            (Order::Interrupt, Turn::Idle, _) => {
                Ok(CommandResult::Rejected(Rejection::NoTurnRunning))
            }
            (Order::Interrupt, Turn::Starting(_), _) => {
                tracing::info!(
                    session = self.id.0,
                    "holding the interrupt until the turn starts"
                );
                self.turn = Turn::Starting(Held::Interrupt);
                Ok(CommandResult::Accepted)
            }
            (Order::Interrupt, Turn::Running(native), _) => {
                let sent = self.interrupt(native).await;
                self.note(sent).await?;
                Ok(CommandResult::Accepted)
            }
            (Order::Reply { request, reply, by }, _, _) => self.reply(request, reply, by).await,
        }
    }

    async fn reply(
        &mut self,
        request: RequestId,
        reply: Reply,
        by: ClientName,
    ) -> Result<CommandResult, Stopped> {
        match self.requests.get(&request).cloned() {
            None | Some(Pending::Resolved(Resolution::Withdrawn)) => {
                Ok(CommandResult::Rejected(Rejection::UnknownRequest))
            }
            Some(Pending::Resolved(
                Resolution::Decided { by: first, .. } | Resolution::Answered { by: first, .. },
            )) => Ok(CommandResult::Rejected(Rejection::AlreadyResolved {
                by: first,
            })),
            Some(Pending::Open { native, asked }) => match fits(&asked, &reply) {
                Err(reason) => Ok(CommandResult::Rejected(Rejection::WrongReply { reason })),
                Ok(()) => {
                    if let (Asked::Approval { call, .. }, Reply::Decide(Decision::Deny)) =
                        (asked, &reply)
                    {
                        self.declined.insert(call);
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
                    self.emit(EventKind::RequestResolved {
                        request,
                        resolution,
                    })
                    .await?;
                    let sent = self.answer(native, reply, &by).await;
                    self.note(sent).await?;
                    Ok(CommandResult::Accepted)
                }
            },
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
            Agent::Down => Err(AgentError::Down),
        }
    }

    async fn interrupt(&self, turn: &NativeTurn) -> Result<(), AgentError> {
        match &self.agent {
            Agent::Claude { session, .. } => Ok(session.interrupt().await?),
            Agent::Codex { thread, .. } => Ok(thread.interrupt(TurnId(turn.0.clone())).await?),
            Agent::Down => Err(AgentError::Down),
        }
    }

    async fn answer(
        &self,
        native: NativeRequest,
        reply: Reply,
        by: &ClientName,
    ) -> Result<(), AgentError> {
        match &self.agent {
            Agent::Claude { session, .. } => Ok(session
                .respond(
                    PermissionRequestId(native.0),
                    mapping::claude_answer(reply, by),
                )
                .await?),
            Agent::Codex { thread, .. } => Ok(thread
                .respond(ApprovalId(native.0), mapping::codex_answer(reply))
                .await?),
            Agent::Down => Err(AgentError::Down),
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

    /// Applies the steps in order.
    async fn apply(&mut self, steps: Vec<Step>) -> Result<(), Stopped> {
        for step in steps {
            self.step(step).await?;
        }
        Ok(())
    }

    async fn step(&mut self, step: Step) -> Result<(), Stopped> {
        match step {
            Step::Emit(kind) => self.emit(kind).await,
            Step::Open {
                native,
                call,
                tool,
                reason,
                for_session,
            } => {
                let asked = Asked::Approval {
                    call: call.clone(),
                    for_session: for_session.clone(),
                };
                let request = self.open(native, asked);
                self.emit(EventKind::RequestOpened {
                    request,
                    call,
                    tool,
                    reason,
                    for_session,
                })
                .await
            }
            Step::Ask { native, questions } => {
                let asked = Asked::Question {
                    count: questions.len(),
                };
                let request = self.open(native, asked);
                self.emit(EventKind::QuestionAsked { request, questions })
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
                self.conversation = self.agent.conversation();
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
            // Nothing the gone agent asked can be answered, and a turn it
            // ran is over; clients mark both when they see the close.
            Step::Closed(reason) => {
                self.agent = Agent::Down;
                self.turn = Turn::Idle;
                self.declined.clear();
                self.requests = std::mem::take(&mut self.requests)
                    .into_iter()
                    .map(|(request, pending)| match pending {
                        Pending::Open { .. } => (request, Pending::Resolved(Resolution::Withdrawn)),
                        resolved => (request, resolved),
                    })
                    .collect();
                self.emit(EventKind::SessionClosed { reason }).await
            }
        }
    }

    /// Stores a new request open under the next id.
    fn open(&mut self, native: NativeRequest, asked: Asked) -> RequestId {
        let request = self.last_request.next();
        self.last_request = request;
        self.requests
            .insert(request, Pending::Open { native, asked });
        request
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

#[cfg(test)]
mod tests {
    use proto::{Answer, Decision, ToolCallId};

    use super::{Asked, Reply, fits};

    fn approval(for_session: Option<&str>) -> Asked {
        Asked::Approval {
            call: ToolCallId(String::from("toolu_1")),
            for_session: for_session.map(String::from),
        }
    }

    #[test]
    fn a_reply_must_fit_what_the_request_asked() {
        let one = Vec::from([Answer(Vec::from([String::from("Blue")]))]);
        assert_eq!(
            [
                fits(&approval(None), &Reply::Decide(Decision::ApproveForSession)),
                fits(
                    &approval(Some("Bash(ls)")),
                    &Reply::Decide(Decision::ApproveForSession)
                ),
                fits(&approval(None), &Reply::Answer(one.clone())),
                fits(&Asked::Question { count: 1 }, &Reply::Answer(one)),
                fits(&Asked::Question { count: 2 }, &Reply::Answer(Vec::new())),
            ],
            [
                Err(String::from(
                    "the agent offers no approval for the session here"
                )),
                Ok(()),
                Err(String::from("an approval takes a decision, not answers")),
                Ok(()),
                Err(String::from("0 answers for 2 questions")),
            ]
        );
    }
}
