//! The new-session form: what came of the last attempt to start a session.

use proto::{Availability, CommandResult, Effort, ModelId, Models, Provider, Rejection};

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

/// What the host offers for a provider. Nothing until it has said, or
/// when it can't run the provider.
pub fn models(availability: Option<&Availability>) -> Models {
    match availability {
        Some(Availability::Available { models, .. }) => models.clone(),
        Some(Availability::Unavailable { .. }) | None => Models::none(),
    }
}

/// One entry of the model picker: what it sends (`None` for the default)
/// and its label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPick {
    pub value: Option<ModelId>,
    pub label: String,
}

/// One entry of the effort picker, as [`ModelPick`] is of the model picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffortPick {
    pub value: Option<Effort>,
    pub label: String,
}

/// The model picker: the host's default first, then every model it offers.
pub fn model_picks(models: &Models) -> Vec<ModelPick> {
    let default = match &models.default {
        Some(id) => format!("Default ({})", model_name(models, id)),
        None => String::from("Default"),
    };
    std::iter::once(ModelPick {
        value: None,
        label: default,
    })
    .chain(models.choices.iter().map(|choice| ModelPick {
        value: Some(choice.id.clone()),
        label: choice.name.clone(),
    }))
    .collect()
}

/// The effort picker for `model` (`None`: the default model): the model's
/// own default first, then each effort it takes.
pub fn effort_picks(models: &Models, model: Option<&ModelId>) -> Vec<EffortPick> {
    let efforts = model
        .or(models.default.as_ref())
        .and_then(|id| models.find(id))
        .map(|choice| choice.efforts.clone())
        .unwrap_or_default();
    std::iter::once(EffortPick {
        value: None,
        label: String::from("Default"),
    })
    .chain(efforts.into_iter().map(|effort| EffortPick {
        value: Some(effort),
        label: String::from(effort.wire()),
    }))
    .collect()
}

/// The effort to keep when the model changes to `model`: `effort` if the
/// model takes it, else the model's default.
pub fn kept_effort(
    models: &Models,
    model: Option<&ModelId>,
    effort: Option<Effort>,
) -> Option<Effort> {
    effort_picks(models, model)
        .into_iter()
        .find(|pick| pick.value == effort)
        .and_then(|pick| pick.value)
}

/// A model's name as the host lists it, or its id if the host doesn't.
pub fn model_name(models: &Models, id: &ModelId) -> String {
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
    use proto::{
        Auth, Availability, CommandResult, Effort, ModelChoice, ModelId, Models, Provider,
        Rejection,
    };

    use super::{
        Attempt, EffortPick, ProviderNote, Refusal, effort_picks, kept_effort, model_picks, note,
    };

    #[test]
    fn the_form_says_why_a_provider_is_unavailable_and_that_codex_writes_unasked() {
        let unavailable = Availability::Unavailable {
            reason: String::from("codex is not on PATH"),
        };
        let available = Availability::Available {
            auth: Auth::SignedIn,
            models: Models::none(),
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

    fn labels(picks: Vec<EffortPick>) -> Vec<String> {
        picks.into_iter().map(|pick| pick.label).collect()
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
                model_picks(&models)
                    .into_iter()
                    .map(|pick| (pick.value.map(|id| id.0), pick.label))
                    .collect::<Vec<(Option<String>, String)>>(),
                labels(effort_picks(&models, None)),
                labels(effort_picks(&models, Some(&haiku))),
                labels(effort_picks(&Models::none(), None)),
                kept_effort(&models, Some(&haiku), Some(Effort::Max)),
                kept_effort(&models, Some(&haiku), Some(Effort::High)),
            ),
            (
                Vec::from([
                    (None, String::from("Default (Opus)")),
                    (Some(String::from("opus")), String::from("Opus")),
                    (Some(String::from("haiku")), String::from("Haiku")),
                ]),
                Vec::from([
                    String::from("Default"),
                    String::from("low"),
                    String::from("max")
                ]),
                Vec::from([String::from("Default"), String::from("high")]),
                Vec::from([String::from("Default")]),
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
