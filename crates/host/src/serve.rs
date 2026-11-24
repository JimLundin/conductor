//! What a session command from the client link asks of the host.

use crate::registry::InstanceId;
use crate::session::{SessionCommand, SessionId, StartAt};
use agents::Reply;
use agents::{ClientName, Effort, ModelId, Models};

use crate::session::Order;

/// An order that relaunches the session's agent first, and takes the
/// models its provider offers now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relaunch {
    Continue,
    Effort(Effort),
}

impl Relaunch {
    #[must_use]
    pub const fn order(self, offered: Models) -> Order {
        match self {
            Self::Continue => Order::Start { offered },
            Self::Effort(effort) => Order::Effort(effort, offered),
        }
    }
}

/// What a session command asks of the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionRoute {
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
    /// Finishes the session on its issue, in the background.
    Finish(SessionId, ClientName),
}

/// What `command`, from the client `by`, asks of the host.
#[must_use]
pub fn session(command: SessionCommand, by: ClientName) -> SessionRoute {
    match command {
        SessionCommand::Start {
            instance,
            at,
            model,
            effort,
        } => SessionRoute::Start {
            instance,
            at,
            choice: (model, effort),
            by,
        },
        SessionCommand::SendMessage { session, text } => {
            SessionRoute::Order(session, Order::Send { text, by })
        }
        SessionCommand::Interrupt { session } => {
            SessionRoute::Order(session, Order::Interrupt)
        }
        SessionCommand::Continue { session } => {
            SessionRoute::Relaunch(session, Relaunch::Continue)
        }
        SessionCommand::Resolve {
            session,
            request,
            decision,
        } => {
            let reply = Reply::Decide(decision);
            SessionRoute::Order(session, Order::Reply { request, reply, by })
        }
        SessionCommand::Answer {
            session,
            request,
            answers,
        } => {
            let reply = Reply::Answer(answers);
            SessionRoute::Order(session, Order::Reply { request, reply, by })
        }
        SessionCommand::GetDiff { session, snapshot } => {
            SessionRoute::Order(session, Order::Diff(snapshot))
        }
        SessionCommand::SetEffort { session, effort } => {
            SessionRoute::Relaunch(session, Relaunch::Effort(effort))
        }
        SessionCommand::Remove { session } => {
            SessionRoute::Order(session, Order::Remove)
        }
        SessionCommand::Finish { session } => SessionRoute::Finish(session, by),
    }
}

#[cfg(test)]
mod tests;
