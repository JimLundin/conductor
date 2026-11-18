//! The new-session form: what came of the last attempt to start a session.

use crate::prelude::*;
use proto::{
    Availability, CommandResult, Effort, ModelId, Models, ProviderInfo,
    Rejection,
};

/// What the form says under the chosen instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderNote {
    Nothing,
    /// The host can't run it, for this reason.
    Unavailable(String),
    /// What the provider's plugin says of it, such as what its agent does
    /// without asking.
    Says(String),
}

/// `info` and `availability` are `None` until the host has said.
#[must_use]
pub fn note(
    info: Option<&ProviderInfo>,
    availability: Option<&Availability>,
) -> ProviderNote {
    match (availability, info.and_then(|known| known.note.clone())) {
        (Some(Availability::Unavailable { reason }), _) => {
            ProviderNote::Unavailable(reason.clone())
        }
        (Some(Availability::Available { .. }) | None, Some(text)) => {
            ProviderNote::Says(text)
        }
        (Some(Availability::Available { .. }) | None, None) => {
            ProviderNote::Nothing
        }
    }
}

/// What the host offers for an instance. Nothing until it has said, or
/// when it can't run the instance.
#[must_use]
pub fn models(availability: Option<&Availability>) -> Models {
    match availability {
        Some(Availability::Available { models, .. }) => models.clone(),
        Some(Availability::Unavailable { .. }) | None => Models::none(),
    }
}

/// The label of the model picker's first entry, which sends no model.
#[must_use]
pub fn default_label(models: &Models) -> String {
    models.default.as_ref().map_or_else(
        || String::from("Default"),
        |id| format!("Default ({})", model_name(models, id)),
    )
}

/// The efforts `model` takes (`None`: the default model), lowest first.
#[must_use]
pub fn efforts(models: &Models, model: Option<&ModelId>) -> Vec<Effort> {
    model
        .or(models.default.as_ref())
        .and_then(|id| models.find(id))
        .map_or(Vec::new(), |choice| choice.efforts.clone())
}

/// The effort to keep when the model changes to `model`: `effort` if the
/// model takes it, else the model's default.
#[must_use]
pub fn kept_effort(
    models: &Models,
    model: Option<&ModelId>,
    effort: Option<Effort>,
) -> Option<Effort> {
    effort.filter(|kept| efforts(models, model).contains(kept))
}

/// A model's name as the host lists it, or its id if the host doesn't.
fn model_name(models: &Models, id: &ModelId) -> String {
    models
        .find(id)
        .map_or_else(|| id.0.clone(), |choice| choice.name.clone())
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
    Instance(String),
    Other(String),
}

impl Attempt {
    /// The host's answer to `StartSession`, or the reason it never arrived.
    #[must_use]
    pub fn answered(answer: Result<CommandResult, String>) -> Self {
        match answer {
            Ok(
                CommandResult::Accepted
                | CommandResult::Started { .. }
                | CommandResult::Instances(_)
                | CommandResult::Diff(_),
            ) => Self::Editing,
            Ok(CommandResult::Rejected(Rejection::NoSuchDirectory)) => {
                Self::Refused(Refusal::Directory)
            }
            Ok(CommandResult::Rejected(Rejection::InstanceUnavailable {
                reason,
            })) => Self::Refused(Refusal::Instance(reason)),
            Ok(CommandResult::Rejected(rejection)) => {
                Self::Refused(Refusal::Other(rejection.label()))
            }
            Err(error) => Self::Refused(Refusal::Other(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use proto::{
        Auth, Availability, CommandResult, Effort, ModelChoice, ModelId,
        Models, ProviderInfo, Rejection,
    };

    use super::{
        Attempt, ProviderNote, Refusal, default_label, efforts, kept_effort,
        note,
    };

    #[test]
    fn the_form_says_why_a_provider_is_unavailable_else_its_plugin_s_note() {
        let unavailable = Availability::Unavailable {
            reason: String::from("not on PATH"),
        };
        let available = Availability::Available {
            auth: Auth::SignedIn,
            models: Models::none(),
        };
        let info = |note: Option<&str>| ProviderInfo {
            label: String::from("Agent"),
            note: note.map(String::from),
            credentials: Vec::new(),
        };
        let noted = info(Some("writes unasked"));
        let quiet = info(None);
        assert_eq!(
            [
                (Some(&noted), Some(&unavailable)),
                (Some(&quiet), Some(&unavailable)),
                (Some(&noted), Some(&available)),
                (Some(&noted), None),
                (Some(&quiet), Some(&available)),
                (None, None),
            ]
            .map(|(info, availability)| note(info, availability)),
            [
                ProviderNote::Unavailable(String::from("not on PATH")),
                ProviderNote::Unavailable(String::from("not on PATH")),
                ProviderNote::Says(String::from("writes unasked")),
                ProviderNote::Says(String::from("writes unasked")),
                ProviderNote::Nothing,
                ProviderNote::Nothing,
            ]
        );
    }

    #[test]
    fn the_pickers_offer_the_default_then_the_models_and_their_efforts() {
        let models = Models {
            choices: Vec::from([
                ModelChoice {
                    id: ModelId(String::from("opus")),
                    name: String::from("Opus"),
                    description: String::from("best"),
                    efforts: Vec::from([Effort::Low, Effort::Max]),
                },
                ModelChoice {
                    id: ModelId(String::from("haiku")),
                    name: String::from("Haiku"),
                    description: String::from("fast"),
                    efforts: Vec::from([Effort::High]),
                },
            ]),
            default: Some(ModelId(String::from("opus"))),
        };
        let haiku = ModelId(String::from("haiku"));
        assert_eq!(
            (
                default_label(&models),
                default_label(&Models::none()),
                efforts(&models, None),
                efforts(&models, Some(&haiku)),
                efforts(&Models::none(), None),
                kept_effort(&models, Some(&haiku), Some(Effort::Max)),
                kept_effort(&models, Some(&haiku), Some(Effort::High)),
            ),
            (
                String::from("Default (Opus)"),
                String::from("Default"),
                Vec::from([Effort::Low, Effort::Max]),
                Vec::from([Effort::High]),
                Vec::new(),
                None,
                Some(Effort::High),
            )
        );
    }

    #[test]
    fn a_refusal_lands_under_its_field() {
        assert_eq!(
            [
                Rejection::NoSuchDirectory,
                Rejection::InstanceUnavailable {
                    reason: String::from("no codex"),
                },
                Rejection::TurnInProgress,
            ]
            .map(|rejection| Attempt::answered(Ok(
                CommandResult::Rejected(rejection)
            ))),
            [
                Attempt::Refused(Refusal::Directory),
                Attempt::Refused(Refusal::Instance(String::from("no codex"))),
                Attempt::Refused(Refusal::Other(String::from(
                    "a turn is already running"
                ))),
            ]
        );
    }
}
