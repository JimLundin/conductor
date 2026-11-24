//! The client link's rules on one connection (architecture §8): who is
//! admitted, what each later stream may carry, which commands keep a
//! receipt, and what each command is called in the host's log lines.

use alloc::string::String;

use proto::fleet::{DeviceHello, FleetCommand, Identity};
use proto::{
    AuthToken, ClientName, CommandId, HomeCommand, InstanceCommand,
    PushCommand, SessionCommand, SignInCommand,
};

use crate::log::Kind;
use crate::{Command, Open, Subscribe};

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

/// On a host alone, the first stream must be a `Hello` that carries the
/// host's `token`.
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

/// On a host in a fleet, the first stream must be a device's hello, whose
/// certificate and proof the fleet checks; anything else is refused.
///
/// # Errors
///
/// The refusal of a first stream that is not a device's hello.
pub fn device_hello(first: Option<Open>) -> Result<DeviceHello, Admission> {
    match first {
        Some(Open::FleetHello(hello)) => Ok(hello),
        Some(Open::Hello(_) | Open::Subscribe(_) | Open::Command { .. })
        | None => Err(device_required()),
    }
}

/// A device's hello is admitted as the owner the fleet found for its
/// certificate, if it found one.
#[must_use]
pub fn device(owner: Option<Identity>) -> Admission {
    owner.map_or_else(device_required, |found| {
        Admission::Admitted(ClientName(found.name))
    })
}

fn device_required() -> Admission {
    refused("a current fleet device certificate is required")
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
            HomeCommand::CreateIssue(_) | HomeCommand::ChangeIssue { .. },
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
            HomeCommand::CreateIssue(_) => "create_issue",
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

#[cfg(test)]
mod tests;
