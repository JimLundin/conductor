//! The form that starts a session: the instance it runs on, its model and
//! effort, and the directory on the host.

use crate::prelude::*;
use client::host_view::HostView;
use client::new_session::{
    Attempt, ProviderNote, Refusal, default_label, efforts, kept_effort,
    models, note,
};
use proto::{
    Availability, Catalog, CommandBody, CommandResult, Effort, HostPath,
    InstanceId, Instances, ModelId,
};

use crate::sessions::short;
use crate::{Effect, Reply, ReplyTo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormIntent {
    /// Another instance offers other models, so the model and effort go
    /// back to their defaults.
    PickInstance(InstanceId),
    /// A model's id; empty for the default.
    PickModel(String),
    /// An effort's word; empty for the default.
    PickEffort(String),
    TypeDirectory(String),
    Start,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSessionForm {
    pub instances: Vec<InstanceChoice>,
    /// What the form says under the instances.
    pub instance_hint: Option<Hint>,
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
pub struct InstanceChoice {
    /// What choosing it does.
    pub pick: FormIntent,
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
    /// `None` until a person picks one: the first available instance.
    instance: Option<InstanceId>,
    model: Option<ModelId>,
    effort: Option<Effort>,
    directory: String,
    attempt: Attempt,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            instance: None,
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
        instances: Option<&Instances>,
        visit: u64,
    ) -> (Self, Vec<Effect>) {
        match intent {
            FormIntent::PickInstance(instance) => (
                Self {
                    instance: Some(instance),
                    model: None,
                    effort: None,
                    ..self
                },
                Vec::new(),
            ),
            FormIntent::PickModel(id) => {
                let offered =
                    models(availability(instances, self.chosen(instances)));
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
                let Some(instance) = self.chosen(instances).cloned() else {
                    return (self, Vec::new());
                };
                let body = CommandBody::StartSession {
                    instance,
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

    /// The instance a person picked, if the host still has it, else the
    /// first available one, else the first.
    fn chosen<'a>(
        &'a self,
        instances: Option<&'a Instances>,
    ) -> Option<&'a InstanceId> {
        let known = instances.map(|listed| &listed.0);
        let picked = self
            .instance
            .as_ref()
            .filter(|id| known.is_some_and(|listed| listed.contains_key(*id)));
        picked.or_else(|| {
            let mut ids = known.into_iter().flatten();
            ids.clone()
                .find(|(_, status)| {
                    matches!(
                        status.availability,
                        Availability::Available { .. }
                    )
                })
                .or_else(|| ids.next())
                .map(|(id, _)| id)
        })
    }

    pub fn screen(
        &self,
        view: &HostView,
        instances: Option<&Instances>,
        catalog: &Catalog,
    ) -> NewSessionForm {
        let (directory_error, instance_error, form_error) = match &self.attempt
        {
            Attempt::Refused(Refusal::Directory) => (
                Some(String::from(
                    "There is no directory at that path on the host.",
                )),
                None,
                None,
            ),
            Attempt::Refused(Refusal::Instance(reason)) => {
                (None, Some(reason.clone()), None)
            }
            Attempt::Refused(Refusal::Other(message)) => {
                (None, None, Some(message.clone()))
            }
            Attempt::Editing | Attempt::Starting => (None, None, None),
        };
        let picked = self.chosen(instances);
        let chosen = availability(instances, picked);
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
            match (&self.attempt, self.directory.trim(), picked) {
                (Attempt::Starting, _, _) => (true, "Starting…"),
                (_, "", _) | (_, _, None) => (true, "Start session"),
                (_, _, Some(_)) => (false, "Start session"),
            };
        let status = picked.and_then(|id| instances?.of(id));
        NewSessionForm {
            instances: instances.map_or_else(Vec::new, |listed| {
                listed
                    .0
                    .iter()
                    .map(|(id, choice)| InstanceChoice {
                        pick: FormIntent::PickInstance(id.clone()),
                        label: choice.label.clone(),
                        state: word(Some(&choice.availability)),
                        checked: Some(id) == picked,
                    })
                    .collect()
            }),
            instance_hint: instance_hint(
                instance_error,
                note(status.and_then(|it| catalog.of(it.provider)), chosen),
                status.map_or("That instance", |it| it.label.as_str()),
            ),
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

/// A refusal of the instance, else why the host can't run it, else what
/// its provider's plugin says of it.
fn instance_hint(
    refused: Option<String>,
    note: ProviderNote,
    label: &str,
) -> Option<Hint> {
    match (refused, note) {
        (Some(reason), _) | (None, ProviderNote::Unavailable(reason)) => {
            Some(alert(format!("{label} can't run on this host: {reason}")))
        }
        (None, ProviderNote::Says(text)) => Some(Hint {
            class: "hint muted",
            role: None,
            text,
        }),
        (None, ProviderNote::Nothing) => None,
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

fn availability<'a>(
    instances: Option<&'a Instances>,
    instance: Option<&InstanceId>,
) -> Option<&'a Availability> {
    instances
        .zip(instance)
        .and_then(|(known, id)| known.availability(id))
}

/// The instance's state in one word, for the form's text and
/// `data-available`.
const fn word(availability: Option<&Availability>) -> &'static str {
    match availability {
        Some(Availability::Available { .. }) => "available",
        Some(Availability::Unavailable { .. }) => "unavailable",
        None => "unknown",
    }
}
