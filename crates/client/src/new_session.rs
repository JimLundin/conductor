//! The new-session form: what came of the last attempt to start a session.

use proto::{Availability, CommandResult, Provider, Rejection};

pub const PROVIDERS: &[Provider] = &[Provider::Claude, Provider::Codex];

pub fn label(provider: Provider) -> String {
    String::from(match provider {
        Provider::Claude => "Claude Code",
        Provider::Codex => "Codex",
    })
}

/// What the form says under the chosen provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderNote {
    Nothing,
    /// The host can't run it, for this reason.
    Unavailable(String),
    /// Codex's sandbox (`WorkspaceWrite`, asking `OnRequest`) lets it run
    /// commands and write in the session's directory and `/tmp` unasked.
    WritesUnasked,
}

/// `availability` is `None` until the host has said.
pub fn note(provider: Provider, availability: Option<&Availability>) -> ProviderNote {
    match (provider, availability) {
        (_, Some(Availability::Unavailable { reason })) => {
            ProviderNote::Unavailable(reason.clone())
        }
        (Provider::Codex, Some(Availability::Available { .. }) | None) => {
            ProviderNote::WritesUnasked
        }
        (Provider::Claude, Some(Availability::Available { .. }) | None) => ProviderNote::Nothing,
    }
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
            Ok(
                CommandResult::Accepted
                | CommandResult::Started { .. }
                | CommandResult::Providers(_),
            ) => Attempt::Editing,
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
    use proto::{Auth, Availability, CommandResult, Provider, Rejection};

    use super::{Attempt, ProviderNote, Refusal, note};

    #[test]
    fn the_form_says_why_a_provider_is_unavailable_and_that_codex_writes_unasked() {
        let unavailable = Availability::Unavailable {
            reason: String::from("codex is not on PATH"),
        };
        let available = Availability::Available {
            auth: Auth::SignedIn,
        };
        assert_eq!(
            [
                (Provider::Codex, Some(&unavailable)),
                (Provider::Claude, Some(&unavailable)),
                (Provider::Codex, Some(&available)),
                (Provider::Codex, None),
                (Provider::Claude, Some(&available)),
            ]
            .map(|(provider, availability)| note(provider, availability)),
            [
                ProviderNote::Unavailable(String::from("codex is not on PATH")),
                ProviderNote::Unavailable(String::from("codex is not on PATH")),
                ProviderNote::WritesUnasked,
                ProviderNote::WritesUnasked,
                ProviderNote::Nothing,
            ]
        );
    }

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
