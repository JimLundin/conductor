use crate::registry::InstanceId;
use crate::session::{SessionCommand, SessionId, StartAt};
use agents::Reply;
use agents::{ClientName, Effort, ModelId, Models};

use crate::session::Order;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionRoute {
    Order(SessionId, Order),
    Relaunch(SessionId, Relaunch),
    Start {
        instance: InstanceId,
        at: StartAt,
        choice: (Option<ModelId>, Option<Effort>),
        by: ClientName,
    },
    Finish(SessionId, ClientName),
}

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
