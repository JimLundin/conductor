//! The protocol's rules on one connection (architecture §8): who is
//! admitted, what each later stream may carry, which commands the command
//! log records, and what each command asks of the host.

use agents::Reply;
use proto::{
    AuthToken, CheckpointId, ClientName, Command, CommandBody, CommandResult,
    Credential, Effort, InstanceId, IssueChange, IssueFields, IssueId, ModelId,
    Models, Open, ProjectId, Provider, Rejection, RequestId, SessionId,
    SignInCode, StartAt, Subscribe, Task, TaskId,
};

use crate::prelude::*;
use crate::session;

/// What the connection's first stream decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    Admitted(ClientName),
    /// The host answers `HelloReply::Refused` with the reason, then closes
    /// the connection.
    Refused {
        reason: String,
    },
}

/// The first stream must be a `Hello` that carries the host's `token`.
#[must_use]
pub fn admit(first: Option<Open>, token: &AuthToken) -> Admission {
    match first {
        Some(Open::Hello(hello)) => hello.token.check(token).map_or_else(
            |_| refused("wrong token"),
            |()| Admission::Admitted(hello.client),
        ),
        Some(Open::Subscribe(_) | Open::Command(_) | Open::FleetHello(_))
        | None => refused("the first stream must be a hello"),
    }
}

fn refused(reason: &str) -> Admission {
    Admission::Refused {
        reason: String::from(reason),
    }
}

/// What a stream after the `Hello` carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stream {
    /// Answered with one `CommandResult`, then finished.
    Command(Command),
    /// Answered with events until the client falls behind or the
    /// connection goes.
    Subscribe(Subscribe),
    /// A second `Hello`, or no frame at all: finished unanswered.
    Finish,
}

#[must_use]
pub fn stream(open: Option<Open>) -> Stream {
    match open {
        Some(Open::Command(command)) => Stream::Command(command),
        Some(Open::Subscribe(subscribe)) => Stream::Subscribe(subscribe),
        Some(Open::Hello(_) | Open::FleetHello(_)) | None => Stream::Finish,
    }
}

/// A command for one session, already routed to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Order {
    Send {
        text: String,
        by: ClientName,
    },
    Handoff {
        task: Task,
        by: ClientName,
    },
    Interrupt,
    Diff(CheckpointId),
    Undo(CheckpointId),
    Reply {
        request: RequestId,
        reply: Reply,
        by: ClientName,
    },
    /// Starts the agent, resuming the conversation if the session has one:
    /// when the session starts, and on `Continue`. The models are what the
    /// provider offers now.
    Start(Models),
    /// Sets the effort of later turns, if the session's model in these
    /// models takes it. The agent gets it when the next turn starts.
    Effort(Effort, Models),
    /// Stops the agent between turns and removes the session's workspace.
    Remove,
}

impl Order {
    #[must_use]
    pub const fn requires_instance(&self) -> bool {
        !matches!(self, Self::Interrupt | Self::Diff(_) | Self::Remove)
    }

    /// The order for the session's state machine. A conversation an undo
    /// forks takes `fork` as its id.
    #[must_use]
    pub fn session(self, fork: String) -> session::Order {
        use session::Order as Core;
        match self {
            Self::Send { text, by } => Core::Send { text, by },
            Self::Handoff { task, by } => Core::Handoff { task, by },
            Self::Interrupt => Core::Interrupt,
            Self::Diff(checkpoint) => Core::Diff(checkpoint),
            Self::Undo(checkpoint) => Core::Undo { checkpoint, fork },
            Self::Reply { request, reply, by } => {
                Core::Reply { request, reply, by }
            }
            Self::Start(offered) => Core::Start { offered },
            Self::Effort(effort, offered) => Core::Effort(effort, offered),
            Self::Remove => Core::Remove,
        }
    }
}

/// An order that relaunches the session's agent first, and takes the
/// models its provider offers now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relaunch {
    Undo(CheckpointId),
    Continue,
    Effort(Effort),
}

impl Relaunch {
    #[must_use]
    pub fn order(self, offered: Models) -> Order {
        match self {
            Self::Undo(checkpoint) => Order::Undo(checkpoint),
            Self::Continue => Order::Start(offered),
            Self::Effort(effort) => Order::Effort(effort, offered),
        }
    }

    /// What the command answers if the relaunched session has stopped.
    /// `None` fails the command closed: an undo may have begun its durable
    /// phases, so its receipt must not be written.
    #[must_use]
    pub const fn closed(self) -> Option<CommandResult> {
        match self {
            Self::Undo(_) => None,
            Self::Continue | Self::Effort(_) => {
                Some(CommandResult::Rejected(Rejection::SessionClosed))
            }
        }
    }
}

/// What a command asks of the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// Hand the running session an order; a session the host doesn't have
    /// answers `UnknownSession`, and one that stopped `SessionClosed`.
    Order(SessionId, Order),
    Relaunch(SessionId, Relaunch),
    Start {
        instance: InstanceId,
        at: StartAt,
        choice: (Option<ModelId>, Option<Effort>),
        by: ClientName,
    },
    Configure(Configure),
    SignIn(SignInOrder),
    Home(HomeOrder),
    /// Finishes the session on its issue, in the background.
    Finish(SessionId, ClientName),
    Handoff {
        from: SessionId,
        to: SessionId,
        task: TaskId,
        by: ClientName,
    },
    Push(PushOrder),
    Move {
        session: SessionId,
        to: proto::fleet::NodeId,
        retry: Option<proto::movement::MoveId>,
    },
    Fleet(FleetAction),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FleetAction {
    Status,
    Invite,
    Revoke(proto::fleet::NodeId),
    Approve(proto::fleet::PairRequest),
    Admit { ticket: proto::Ticket, name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushOrder {
    PublicKey,
    Register(proto::PushSubscription),
}

/// A command about the host's clone of the home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeOrder {
    Sync,
    Clone(ProjectId),
    Create(ProjectId, IssueFields),
    /// The client signs a note the change adds.
    Change(IssueId, IssueChange, ClientName),
}

/// A command about an instance's sign-in from the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInOrder {
    Start(InstanceId),
    Credential {
        instance: InstanceId,
        kind: String,
        label: String,
    },
    Submit(InstanceId, SignInCode),
    Cancel(InstanceId),
}

/// A command that changes the host's instances, and answers what it runs
/// now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Configure {
    Retry(InstanceId),
    SetCredential {
        provider: Provider,
        credential: Credential,
    },
    Remove(InstanceId),
}

impl Configure {
    #[must_use]
    pub const fn removal(&self) -> Option<&InstanceId> {
        match self {
            Self::Remove(instance) => Some(instance),
            Self::Retry(_) | Self::SetCredential { .. } => None,
        }
    }

    #[must_use]
    pub fn instance(&self, catalog: &proto::Catalog) -> Option<InstanceId> {
        match self {
            Self::Retry(id) | Self::Remove(id) => Some(id.clone()),
            Self::SetCredential {
                provider,
                credential,
            } => catalog
                .of(*provider)
                .and_then(|profile| profile.account(credential).ok())
                .map(|account| InstanceId {
                    provider: *provider,
                    account,
                }),
        }
    }

    #[must_use]
    pub const fn credential(&self) -> Option<(Provider, &Credential)> {
        match self {
            Self::SetCredential {
                provider,
                credential,
            } => Some((*provider, credential)),
            Self::Retry(_) | Self::Remove(_) => None,
        }
    }
}

/// How the command log keeps a command's receipt, so a retry with the same
/// id answers the first result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ledger {
    /// A read, a sign-in command, whose code must reach no store, or a
    /// sync or clone of the home, which brings it up to date whenever it
    /// runs: run it every time.
    Unrecorded,
    /// Runs alone, with its events in one transaction with its receipt.
    Session,
    /// An instance or issue command, which a retry must not repeat: runs
    /// beside session commands; a retry waits for the first.
    Receipt,
}

impl Route {
    #[must_use]
    pub const fn ledger(&self) -> Ledger {
        match self {
            Self::Order(_, Order::Diff(_))
            | Self::SignIn(_)
            | Self::Push(_)
            | Self::Move { .. }
            | Self::Fleet(_)
            | Self::Home(HomeOrder::Sync | HomeOrder::Clone(_)) => {
                Ledger::Unrecorded
            }
            Self::Configure(_)
            | Self::Finish(..)
            | Self::Home(HomeOrder::Create(..) | HomeOrder::Change(..)) => {
                Ledger::Receipt
            }
            Self::Order(..)
            | Self::Relaunch(..)
            | Self::Start { .. }
            | Self::Handoff { .. } => Ledger::Session,
        }
    }
}

const fn retry_instance(instance: InstanceId) -> Route {
    Route::Configure(Configure::Retry(instance))
}

#[must_use]
pub fn route(body: CommandBody, by: ClientName) -> Route {
    match body {
        CommandBody::PushPublicKey => Route::Push(PushOrder::PublicKey),
        CommandBody::RegisterPush(subscription) => {
            Route::Push(PushOrder::Register(subscription))
        }
        CommandBody::HandoffTask { from, to, task } => {
            Route::Handoff { from, to, task, by }
        }
        CommandBody::FleetApprove(request) => {
            Route::Fleet(FleetAction::Approve(request))
        }
        CommandBody::FleetAdmit { ticket, name } => {
            Route::Fleet(FleetAction::Admit { ticket, name })
        }
        CommandBody::FleetStatus => Route::Fleet(FleetAction::Status),
        CommandBody::FleetInvite => Route::Fleet(FleetAction::Invite),
        CommandBody::FleetRevoke(node) => {
            Route::Fleet(FleetAction::Revoke(node))
        }
        CommandBody::GetDiff {
            session,
            checkpoint,
        } => Route::Order(session, Order::Diff(checkpoint)),
        CommandBody::UndoTurn {
            session,
            checkpoint,
        } => Route::Relaunch(session, Relaunch::Undo(checkpoint)),
        CommandBody::StartSession {
            instance,
            at,
            model,
            effort,
        } => Route::Start {
            instance,
            at,
            choice: (model, effort),
            by,
        },
        CommandBody::SyncHome => Route::Home(HomeOrder::Sync),
        CommandBody::CloneProject { project } => {
            Route::Home(HomeOrder::Clone(project))
        }
        CommandBody::Continue { session } => {
            Route::Relaunch(session, Relaunch::Continue)
        }
        CommandBody::CreateIssue { project, fields } => {
            Route::Home(HomeOrder::Create(project, fields))
        }
        CommandBody::ChangeIssue { issue, change } => {
            Route::Home(HomeOrder::Change(issue, change, by))
        }
        CommandBody::FinishSession { session } => Route::Finish(session, by),
        CommandBody::MoveSession { session, to, retry } => {
            Route::Move { session, to, retry }
        }
        CommandBody::RemoveSession { session } => {
            Route::Order(session, Order::Remove)
        }
        CommandBody::SetEffort { session, effort } => {
            Route::Relaunch(session, Relaunch::Effort(effort))
        }
        CommandBody::SendMessage { session, text } => {
            Route::Order(session, Order::Send { text, by })
        }
        CommandBody::Interrupt { session } => {
            Route::Order(session, Order::Interrupt)
        }
        CommandBody::Resolve {
            session,
            request,
            decision,
        } => reply(session, request, Reply::Decide(decision), by),
        CommandBody::Answer {
            session,
            request,
            answers,
        } => reply(session, request, Reply::Answer(answers), by),
        CommandBody::SetCredential {
            provider,
            credential,
        } => set_credential(provider, credential),
        CommandBody::RetryInstance { instance } => retry_instance(instance),
        CommandBody::RemoveInstance { instance } => {
            Route::Configure(Configure::Remove(instance))
        }
        CommandBody::StartCredentialSignIn {
            instance,
            kind,
            label,
        } => credential_sign_in(instance, kind, label),
        CommandBody::StartSignIn { instance } => {
            Route::SignIn(SignInOrder::Start(instance))
        }
        CommandBody::SubmitSignInCode { instance, code } => {
            Route::SignIn(SignInOrder::Submit(instance, code))
        }
        CommandBody::CancelSignIn { instance } => {
            Route::SignIn(SignInOrder::Cancel(instance))
        }
    }
}

const fn set_credential(provider: Provider, credential: Credential) -> Route {
    Route::Configure(Configure::SetCredential {
        provider,
        credential,
    })
}

const fn credential_sign_in(
    instance: InstanceId,
    kind: String,
    label: String,
) -> Route {
    Route::SignIn(SignInOrder::Credential {
        instance,
        kind,
        label,
    })
}

const fn reply(
    session: SessionId,
    request: RequestId,
    reply: Reply,
    by: ClientName,
) -> Route {
    Route::Order(session, Order::Reply { request, reply, by })
}

#[cfg(test)]
mod tests;
