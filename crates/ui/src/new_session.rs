//! The form that starts a session: the agent, its model and effort, and
//! the directory on the host.

use crate::prelude::*;
use client::host_view::HostView;
use client::new_session::{
    Attempt, ProviderNote, Refusal, default_label, efforts, kept_effort, label,
    models, note,
};
use proto::{
    Availability, CommandBody, CommandResult, Effort, HostPath, ModelId,
    Provider, Providers,
};

use crate::sessions::short;
use crate::{Effect, Reply, ReplyTo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormIntent {
    /// Another provider offers other models, so the model and effort go
    /// back to their defaults.
    PickProvider(Provider),
    /// A model's id; empty for the default.
    PickModel(String),
    /// An effort's word; empty for the default.
    PickEffort(String),
    TypeDirectory(String),
    Start,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSessionForm {
    pub providers: Vec<ProviderChoice>,
    /// What the form says under the agents.
    pub provider_hint: Option<Hint>,
    pub models: Vec<Choice>,
    pub efforts: Vec<Choice>,
    pub directory: String,
    pub directory_invalid: bool,
    pub directory_error: Option<Hint>,
    /// The directories sessions ran in, the most recent first.
    pub suggestions: Option<Vec<Suggestion>>,
    pub error: Option<Hint>,
    pub start_disabled: bool,
    pub start_label: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderChoice {
    pub provider: Provider,
    pub label: String,
    /// Whether the host can run it, in one word, also its `data-available`.
    pub state: &'static str,
    pub checked: bool,
}

/// An `option` of a `select`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub label: String,
    pub selected: bool,
    pub disabled: bool,
}

/// A line of help or of trouble under a field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub class: &'static str,
    pub role: Option<&'static str>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    provider: Provider,
    model: Option<ModelId>,
    effort: Option<Effort>,
    directory: String,
    attempt: Attempt,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            provider: Provider::Claude,
            model: None,
            effort: None,
            directory: String::new(),
            attempt: Attempt::Editing,
        }
    }
}

impl Form {
    pub fn update(
        self,
        intent: FormIntent,
        providers: Option<&Providers>,
        visit: u64,
    ) -> (Self, Vec<Effect>) {
        match intent {
            FormIntent::PickProvider(provider) => (
                Self {
                    provider,
                    model: None,
                    effort: None,
                    ..self
                },
                Vec::new(),
            ),
            FormIntent::PickModel(id) => {
                let offered = models(availability(providers, self.provider));
                let picked =
                    offered.find(&ModelId(id)).map(|choice| choice.id.clone());
                (
                    Self {
                        effort: kept_effort(
                            &offered,
                            picked.as_ref(),
                            self.effort,
                        ),
                        model: picked,
                        ..self
                    },
                    Vec::new(),
                )
            }
            FormIntent::PickEffort(word) => (
                Self {
                    effort: Effort::parse(&word),
                    ..self
                },
                Vec::new(),
            ),
            FormIntent::TypeDirectory(directory) => {
                (Self { directory, ..self }, Vec::new())
            }
            FormIntent::Start => {
                let body = CommandBody::StartSession {
                    provider: self.provider,
                    directory: HostPath(self.directory.trim().to_owned()),
                    model: self.model.clone(),
                    effort: self.effort,
                };
                (
                    Self {
                        attempt: Attempt::Starting,
                        ..self
                    },
                    Vec::from([Effect::Send {
                        body,
                        reply: Reply {
                            visit,
                            to: ReplyTo::Start,
                        },
                    }]),
                )
            }
        }
    }

    pub fn answered(self, answer: Result<CommandResult, String>) -> Self {
        Self {
            attempt: Attempt::answered(answer),
            ..self
        }
    }

    pub fn screen(
        &self,
        view: &HostView,
        providers: Option<&Providers>,
    ) -> NewSessionForm {
        let (directory_error, provider_error, form_error) = match &self.attempt
        {
            Attempt::Refused(Refusal::Directory) => (
                Some(String::from(
                    "There is no directory at that path on the host.",
                )),
                None,
                None,
            ),
            Attempt::Refused(Refusal::Provider(reason)) => {
                (None, Some(reason.clone()), None)
            }
            Attempt::Refused(Refusal::Other(message)) => {
                (None, None, Some(message.clone()))
            }
            Attempt::Editing | Attempt::Starting => (None, None, None),
        };
        let chosen = availability(providers, self.provider);
        let offered = models(chosen);
        let suggestions = view
            .directories()
            .iter()
            .map(|path| Suggestion {
                name: short(path).to_owned(),
                path: path.0.clone(),
            })
            .collect::<Vec<Suggestion>>();
        let (start_disabled, start_label) =
            match (&self.attempt, self.directory.trim()) {
                (Attempt::Starting, _) => (true, "Starting…"),
                (_, "") => (true, "Start session"),
                (_, _) => (false, "Start session"),
            };
        NewSessionForm {
            providers: Provider::ALL
                .map(|choice| ProviderChoice {
                    provider: choice,
                    label: label(choice),
                    state: word(availability(providers, choice)),
                    checked: choice == self.provider,
                })
                .to_vec(),
            provider_hint: self
                .provider_hint(provider_error, note(self.provider, chosen)),
            efforts: self
                .effort_choices(&efforts(&offered, self.model.as_ref())),
            models: self.model_choices(&offered),
            directory: self.directory.clone(),
            directory_invalid: directory_error.is_some(),
            directory_error: directory_error.map(alert),
            suggestions: (!suggestions.is_empty()).then_some(suggestions),
            error: form_error.map(alert),
            start_disabled,
            start_label,
        }
    }

    /// A refusal of the provider, else why the host can't run it, else
    /// what Codex does unasked.
    fn provider_hint(
        &self,
        refused: Option<String>,
        note: ProviderNote,
    ) -> Option<Hint> {
        match (refused, note) {
            (Some(reason), _) | (None, ProviderNote::Unavailable(reason)) => {
                Some(alert(format!(
                    "{} can't run on this host: {reason}",
                    label(self.provider)
                )))
            }
            (None, ProviderNote::WritesUnasked) => Some(Hint {
                class: "hint muted",
                role: None,
                text: String::from(
                    "Codex runs commands and edits files in this folder and in /tmp without asking. It asks before writing anywhere else, changing .git, or using the network.",
                ),
            }),
            (None, ProviderNote::Nothing) => None,
        }
    }

    /// The default first, which sends no model, then each model the host
    /// lists.
    fn model_choices(&self, offered: &proto::Models) -> Vec<Choice> {
        let first = Choice {
            value: String::new(),
            label: default_label(offered),
            selected: self.model.is_none(),
            disabled: false,
        };
        let listed = offered.choices.iter().map(|choice| Choice {
            value: choice.id.0.clone(),
            label: choice.name.clone(),
            selected: Some(&choice.id) == self.model.as_ref(),
            disabled: false,
        });
        core::iter::once(first).chain(listed).collect()
    }

    fn effort_choices(&self, offered: &[Effort]) -> Vec<Choice> {
        let first = Choice {
            value: String::new(),
            label: String::from("Default"),
            selected: self.effort.is_none(),
            disabled: false,
        };
        let listed = offered.iter().map(|effort| Choice {
            value: effort.wire().to_owned(),
            label: effort.wire().to_owned(),
            selected: Some(*effort) == self.effort,
            disabled: false,
        });
        core::iter::once(first).chain(listed).collect()
    }
}

/// A line under a field that says what went wrong.
pub const fn alert(text: String) -> Hint {
    Hint {
        class: "hint error",
        role: Some("alert"),
        text,
    }
}

fn availability(
    providers: Option<&Providers>,
    provider: Provider,
) -> Option<&Availability> {
    providers.and_then(|known| known.of(provider))
}

/// The provider's state in one word, for the form's text and
/// `data-available`.
const fn word(availability: Option<&Availability>) -> &'static str {
    match availability {
        Some(Availability::Available { .. }) => "available",
        Some(Availability::Unavailable { .. }) => "unavailable",
        None => "unknown",
    }
}
