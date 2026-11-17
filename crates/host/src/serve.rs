//! The protocol's rules on one connection (architecture §8): who is
//! admitted, what each later stream may carry, which commands the command
//! log records, and what each command asks of the host.

use agents::Reply;
use proto::{
    AuthToken, CheckpointId, ClientName, Command, CommandBody, CommandResult,
    Credential, Effort, HostPath, ModelId, Models, Open, Provider, Rejection,
    RequestId, SessionId, Subscribe,
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
        Some(Open::Subscribe(_) | Open::Command(_)) | None => {
            refused("the first stream must be a hello")
        }
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
        Some(Open::Hello(_)) | None => Stream::Finish,
    }
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
}

impl Order {
    /// The order for the session's state machine. A conversation an undo
    /// forks takes `fork` as its id.
    #[must_use]
    pub fn session(self, fork: String) -> session::Order {
        use session::Order as Core;
        match self {
            Self::Send { text, by } => Core::Send { text, by },
            Self::Interrupt => Core::Interrupt,
            Self::Diff(checkpoint) => Core::Diff(checkpoint),
            Self::Undo(checkpoint) => Core::Undo { checkpoint, fork },
            Self::Reply { request, reply, by } => {
                Core::Reply { request, reply, by }
            }
            Self::Start(offered) => Core::Start { offered },
            Self::Effort(effort, offered) => Core::Effort(effort, offered),
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
        provider: Provider,
        directory: HostPath,
        choice: (Option<ModelId>, Option<Effort>),
        by: ClientName,
    },
    SetCredential(Credential),
    ClearCredential(Provider),
}

/// How the command log keeps a command's receipt, so a retry with the same
/// id answers the first result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ledger {
    /// A read: run it every time.
    Unrecorded,
    /// Runs alone, with its events in one transaction with its receipt.
    Session,
    /// Runs beside session commands; a retry waits for the first.
    Credential,
}

impl Route {
    #[must_use]
    pub const fn ledger(&self) -> Ledger {
        match self {
            Self::Order(_, Order::Diff(_)) => Ledger::Unrecorded,
            Self::SetCredential(_) | Self::ClearCredential(_) => {
                Ledger::Credential
            }
            Self::Order(..) | Self::Relaunch(..) | Self::Start { .. } => {
                Ledger::Session
            }
        }
    }
}

#[must_use]
pub fn route(body: CommandBody, by: ClientName) -> Route {
    match body {
        CommandBody::GetDiff {
            session,
            checkpoint,
        } => Route::Order(session, Order::Diff(checkpoint)),
        CommandBody::UndoTurn {
            session,
            checkpoint,
        } => Route::Relaunch(session, Relaunch::Undo(checkpoint)),
        CommandBody::StartSession {
            provider,
            directory,
            model,
            effort,
        } => Route::Start {
            provider,
            directory,
            choice: (model, effort),
            by,
        },
        CommandBody::Continue { session } => {
            Route::Relaunch(session, Relaunch::Continue)
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
        } => Route::Order(
            session,
            Order::Reply {
                request,
                reply: Reply::Decide(decision),
                by,
            },
        ),
        CommandBody::Answer {
            session,
            request,
            answers,
        } => Route::Order(
            session,
            Order::Reply {
                request,
                reply: Reply::Answer(answers),
                by,
            },
        ),
        CommandBody::SetCredential { credential } => {
            Route::SetCredential(credential)
        }
        CommandBody::ClearCredential { provider } => {
            Route::ClearCredential(provider)
        }
    }
}

#[cfg(test)]
mod tests;
