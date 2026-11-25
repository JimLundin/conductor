use alloc::string::{String, ToString};
use core::fmt::Display;

use crate::AuthToken;
use crate::CommandId;
use agents::ClientName;
use home::canvas::{CanvasFile, CanvasKey, CanvasName};
use home::{Home, HomeCommand, HomeRefusal, HomeResult, IssueId};
use host::fleet::{DeviceHello, FleetCommand, Identity};
use host::push::PushCommand;
use host::registry::InstanceCommand;
use host::session::SessionCommand;
use host::sign_in::SignInCommand;

use crate::log::Kind;
use crate::{Command, Open, Subscribe};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    Admitted(ClientName),
    Refused { reason: String },
}

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

pub fn device_hello(first: Option<Open>) -> Result<DeviceHello, Admission> {
    match first {
        Some(Open::FleetHello(hello)) => Ok(hello),
        Some(Open::Hello(_) | Open::Subscribe(_) | Open::Command { .. })
        | None => Err(device_required()),
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stream {
    Command(CommandId, Command),
    Subscribe(Subscribe),
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

#[must_use]
pub const fn ledger(command: &Command) -> Option<Kind> {
    match command {
        Command::Session(SessionCommand::GetDiff { .. })
        | Command::SignIn(_)
        | Command::Push(_)
        | Command::Fleet(_)
        | Command::Home(
            HomeCommand::Sync
            | HomeCommand::Clone { .. }
            | HomeCommand::Canvas { .. },
        ) => None,
        Command::Instance(_)
        | Command::Session(SessionCommand::Finish { .. })
        | Command::Home(
            HomeCommand::CreateIssue { .. } | HomeCommand::ChangeIssue { .. },
        ) => Some(Kind::Receipt),
        Command::Session(_) => Some(Kind::Session),
    }
}

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
            HomeCommand::Canvas { .. } => "get_canvas",
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

pub fn canvas_key(
    home: Option<&Home>,
    issue: IssueId,
    name: CanvasName,
) -> Result<CanvasKey, HomeRefusal> {
    let known = home.ok_or(HomeRefusal::NoHome)?;
    let project = known
        .projects
        .iter()
        .find(|project| project.issues.iter().any(|at| at.id() == &issue))
        .ok_or(HomeRefusal::UnknownIssue)?;
    Ok(CanvasKey {
        project: project.id,
        issue,
        name,
    })
}

#[must_use]
pub fn kept_canvas<E: Display>(
    kept: Result<Option<CanvasFile>, E>,
) -> HomeResult {
    match kept {
        Ok(Some(file)) => HomeResult::Canvas(file),
        Ok(None) => HomeResult::Refused(HomeRefusal::UnknownCanvas),
        Err(error) => HomeResult::Refused(HomeRefusal::CanvasUnavailable {
            reason: error.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests;
