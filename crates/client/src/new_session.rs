//! The new-session form: which providers the host can run, and what came of
//! the last attempt. The host tells a client about a provider only by
//! answering `StartSession`, so availability is learned from its answers and
//! from the sessions it has run.

use proto::{CloseReason, CommandResult, Provider, Rejection};

use crate::host_view::{HostView, SessionStatus};

pub const PROVIDERS: &[Provider] = &[Provider::Claude, Provider::Codex];

pub fn label(provider: Provider) -> String {
    String::from(match provider {
        Provider::Claude => "Claude Code",
        Provider::Codex => "Codex",
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartForm {
    pub attempt: Attempt,
    unavailable: Vec<Unavailable>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// A session of this provider has run on the host.
    Available,
    /// The host refused to start it, for this reason.
    Unavailable(String),
    Unknown,
}

impl Availability {
    /// The word the form shows, and its `data-available`.
    pub fn word(&self) -> String {
        String::from(match self {
            Availability::Available => "available",
            Availability::Unavailable(_) => "unavailable",
            Availability::Unknown => "unknown",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Unavailable {
    provider: Provider,
    reason: String,
}

impl StartForm {
    pub fn empty() -> StartForm {
        StartForm {
            attempt: Attempt::Editing,
            unavailable: Vec::new(),
        }
    }

    pub fn attempt(self, attempt: Attempt) -> StartForm {
        StartForm { attempt, ..self }
    }

    /// Folds the host's answer to `StartSession` for `provider`, or the
    /// reason it never arrived.
    pub fn answered(self, provider: Provider, answer: Result<CommandResult, String>) -> StartForm {
        match answer {
            Ok(CommandResult::Accepted | CommandResult::Started { .. }) => {
                self.attempt(Attempt::Editing)
            }
            Ok(CommandResult::Rejected(Rejection::NoSuchDirectory)) => {
                self.attempt(Attempt::Refused(Refusal::Directory))
            }
            Ok(CommandResult::Rejected(Rejection::ProviderUnavailable { reason })) => StartForm {
                attempt: Attempt::Refused(Refusal::Provider(reason.clone())),
                unavailable: self
                    .unavailable
                    .into_iter()
                    .filter(|known| known.provider != provider)
                    .chain([Unavailable { provider, reason }])
                    .collect(),
            },
            Ok(CommandResult::Rejected(rejection)) => {
                self.attempt(Attempt::Refused(Refusal::Other(rejection.label())))
            }
            Err(error) => self.attempt(Attempt::Refused(Refusal::Other(error))),
        }
    }

    pub fn availability(&self, host: &HostView, provider: Provider) -> Availability {
        let refused = self
            .unavailable
            .iter()
            .find(|known| known.provider == provider);
        let ran = host.sessions.values().find(|session| {
            session.provider == provider
                && matches!(
                    session.status,
                    SessionStatus::Idle
                        | SessionStatus::Running
                        | SessionStatus::Closed(CloseReason::Exited { .. })
                )
        });
        match (refused, ran) {
            (Some(known), _) => Availability::Unavailable(known.reason.clone()),
            (None, Some(_)) => Availability::Available,
            (None, None) => Availability::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use proto::{
        ClientName, CommandResult, Event, EventKind, HostPath, HostSeq, Provider, Rejection,
        SessionId,
    };

    use super::{Attempt, Availability, Refusal, StartForm};
    use crate::host_view::HostView;

    fn refused(reason: &str) -> Result<CommandResult, String> {
        Ok(CommandResult::Rejected(Rejection::ProviderUnavailable {
            reason: String::from(reason),
        }))
    }

    #[test]
    fn a_refusal_lands_under_its_field_and_marks_the_provider() {
        let host = Vec::from([
            EventKind::SessionCreated {
                provider: Provider::Claude,
                directory: HostPath(String::from("/a")),
                by: ClientName(String::from("desk")),
            },
            EventKind::SessionReady,
        ])
        .into_iter()
        .map(|kind| Event {
            seq: HostSeq(1),
            session: SessionId(1),
            kind,
        })
        .fold(HostView::empty(), HostView::apply);
        let form = StartForm::empty()
            .answered(Provider::Codex, refused("no codex"))
            .answered(Provider::Codex, refused("still no codex"));
        assert_eq!(
            form.attempt,
            Attempt::Refused(Refusal::Provider(String::from("still no codex")))
        );
        assert_eq!(
            [
                form.availability(&host, Provider::Claude),
                form.availability(&host, Provider::Codex),
                StartForm::empty().availability(&HostView::empty(), Provider::Claude),
            ],
            [
                Availability::Available,
                Availability::Unavailable(String::from("still no codex")),
                Availability::Unknown,
            ]
        );
        assert_eq!(
            form.answered(
                Provider::Claude,
                Ok(CommandResult::Rejected(Rejection::NoSuchDirectory))
            )
            .attempt,
            Attempt::Refused(Refusal::Directory)
        );
    }
}
