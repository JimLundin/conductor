//! The client link's rules on one connection (architecture §8): who is
//! admitted, what each later stream may carry, which commands the command
//! log records, and what a session command asks of the host.

use agents::Reply;
use home::HomeCommand;
use proto::fleet::FleetCommand;
use proto::{
    AuthToken, ClientName, Command, CommandId, Effort, InstanceCommand,
    InstanceId, ModelId, Models, Open, PushCommand, SessionCommand, SessionId,
    SignInCommand, StartAt, Subscribe,
};

use crate::log::Kind;
use crate::prelude::*;
use crate::session::Order;

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
        Some(
            Open::Subscribe(_) | Open::Command { .. } | Open::FleetHello(_),
        )
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
    Command(CommandId, Command),
    /// Answered with events until the client falls behind or the
    /// connection goes.
    Subscribe(Subscribe),
    /// A second `Hello`, or no frame at all: finished unanswered.
    Finish,
}

#[must_use]
pub fn stream(open: Option<Open>) -> Stream {
    match open {
        Some(Open::Command { id, body }) => Stream::Command(id, body),
        Some(Open::Subscribe(subscribe)) => Stream::Subscribe(subscribe),
        Some(Open::Hello(_) | Open::FleetHello(_)) | None => Stream::Finish,
    }
}

/// Which log a command's receipt goes to, if it keeps one: sign-ins, push,
/// the fleet, a diff and the home's sync and clone run every time.
#[must_use]
pub const fn ledger(command: &Command) -> Option<Kind> {
    match command {
        Command::Session(SessionCommand::GetDiff { .. })
        | Command::SignIn(_)
        | Command::Push(_)
        | Command::Fleet(_)
        | Command::Home(HomeCommand::Sync | HomeCommand::Clone { .. }) => None,
        Command::Instance(_)
        | Command::Session(SessionCommand::Finish { .. })
        | Command::Home(
            HomeCommand::CreateIssue { .. } | HomeCommand::ChangeIssue { .. },
        ) => Some(Kind::Receipt),
        Command::Session(_) => Some(Kind::Session),
    }
}

/// What a command is called in the host's log lines, which name no
/// payload.
#[must_use]
pub const fn operation(command: &Command) -> &'static str {
    match command {
        Command::Session(asked) => match asked {
            SessionCommand::Start { .. } => "start_session",
            SessionCommand::SendMessage { .. } => "send_message",
            SessionCommand::Interrupt { .. } => "interrupt",
            SessionCommand::Continue { .. } => "continue",
            SessionCommand::Resolve { .. } => "resolve",
            SessionCommand::Answer { .. } => "answer",
            SessionCommand::GetDiff { .. } => "get_diff",
            SessionCommand::SetEffort { .. } => "set_effort",
            SessionCommand::Remove { .. } => "remove_session",
            SessionCommand::Finish { .. } => "finish_session",
        },
        Command::Instance(asked) => match asked {
            InstanceCommand::SetCredential { .. } => "set_credential",
            InstanceCommand::Remove { .. } => "remove_instance",
        },
        Command::SignIn(asked) => match asked {
            SignInCommand::Start { .. } => "start_sign_in",
            SignInCommand::StartCredential { .. } => "start_credential_sign_in",
            SignInCommand::SubmitCode { .. } => "submit_sign_in_code",
            SignInCommand::Cancel { .. } => "cancel_sign_in",
        },
        Command::Home(asked) => match asked {
            HomeCommand::Sync => "sync_home",
            HomeCommand::Clone { .. } => "clone_project",
            HomeCommand::CreateIssue { .. } => "create_issue",
            HomeCommand::ChangeIssue { .. } => "change_issue",
        },
        Command::Push(asked) => match asked {
            PushCommand::PublicKey => "push_public_key",
            PushCommand::Register(_) => "register_push",
        },
        Command::Fleet(asked) => match asked {
            FleetCommand::Status => "fleet_status",
            FleetCommand::Invite => "fleet_invite",
            FleetCommand::Revoke(_) => "fleet_revoke",
            FleetCommand::Approve(_) => "fleet_approve",
            FleetCommand::Admit { .. } => "fleet_admit",
        },
    }
}

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
