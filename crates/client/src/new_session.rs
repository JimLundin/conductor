//! The new-session form: what came of the last attempt to start a session.

use proto::{CommandResult, Provider, Rejection};

pub const PROVIDERS: &[Provider] = &[Provider::Claude, Provider::Codex];

pub fn label(provider: Provider) -> String {
    String::from(match provider {
        Provider::Claude => "Claude Code",
        Provider::Codex => "Codex",
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attempt {
    Editing,
    Starting,
    Refused(Refusal),
}

/// Where the form shows a refusal: under the field it is about, or under
/// the whole form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Directory,
    Provider(String),
    Other(String),
}

impl Attempt {
    /// The host's answer to `StartSession`, or the reason it never arrived.
    pub fn answered(answer: Result<CommandResult, String>) -> Attempt {
        match answer {
            Ok(CommandResult::Accepted | CommandResult::Started { .. }) => Attempt::Editing,
            Ok(CommandResult::Rejected(Rejection::NoSuchDirectory)) => {
                Attempt::Refused(Refusal::Directory)
            }
            Ok(CommandResult::Rejected(Rejection::ProviderUnavailable { reason })) => {
                Attempt::Refused(Refusal::Provider(reason))
            }
            Ok(CommandResult::Rejected(rejection)) => {
                Attempt::Refused(Refusal::Other(rejection.label()))
            }
            Err(error) => Attempt::Refused(Refusal::Other(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use proto::{CommandResult, Rejection};

    use super::{Attempt, Refusal};

    #[test]
    fn a_refusal_lands_under_its_field() {
        assert_eq!(
            [
                Rejection::NoSuchDirectory,
                Rejection::ProviderUnavailable {
                    reason: String::from("no codex"),
                },
                Rejection::TurnInProgress,
            ]
            .map(|rejection| Attempt::answered(Ok(CommandResult::Rejected(rejection)))),
            [
                Attempt::Refused(Refusal::Directory),
                Attempt::Refused(Refusal::Provider(String::from("no codex"))),
                Attempt::Refused(Refusal::Other(String::from("a turn is already running"))),
            ]
        );
    }
}
