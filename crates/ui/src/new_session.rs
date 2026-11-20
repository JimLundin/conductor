//! The form that starts a session: the host it runs on, then one of that
//! host's instances, its model and effort, and one of the host's projects
//! or a directory there; or, opened from an issue, the issue.

use alloc::collections::BTreeMap;

use crate::prelude::*;
use proto::{
    Availability, CommandBody, CommandResult, Effort, Home, HostPath,
    InstanceId, Instances, IssueId, ModelId, Models, ProjectId, ProviderNote,
    Rejection, StartAt,
};

use crate::host::{Host, HostId, by_name};
use crate::sessions::short;
use crate::{At, Effect, ReplyTo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormIntent {
    /// A host's key. Another host has other instances, so the instance,
    /// model and effort go back to their defaults.
    PickHost(String),
    /// Another instance offers other models, so the model and effort go
    /// back to their defaults.
    PickInstance(InstanceId),
    /// A model's id; empty for the default.
    PickModel(String),
    /// An effort's word; empty for the default.
    PickEffort(String),
    /// A project's id; empty for a directory.
    PickProject(String),
    TypeDirectory(String),
    Start,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSessionForm {
    pub subtitle: String,
    /// The issue the session works on, by its title, when the form was
    /// opened from one; the projects and the directory are hidden then.
    pub issue: Option<String>,
    /// The `select` of the paired hosts, each valued by its key.
    pub hosts: Vec<Choice>,
    pub instances: Vec<InstanceChoice>,
    /// What the form says under the instances.
    pub instance_hint: Option<Hint>,
    pub models: Vec<Choice>,
    pub efforts: Vec<Choice>,
    /// A directory first, then each project the host's home lists; none
    /// on a host with no home.
    pub projects: Option<Vec<Choice>>,
    /// The directory field shows unless a project is picked.
    pub directory_shown: bool,
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
    /// The host of the session shown last, or the one a person picked;
    /// `None` means the first live one.
    host: Option<HostId>,
    /// `None` until a person picks one: the first available instance.
    instance: Option<InstanceId>,
    model: Option<ModelId>,
    effort: Option<Effort>,
    /// `None` starts in the directory.
    project: Option<ProjectId>,
    /// Starts on this issue instead of a project or a directory.
    issue: Option<IssueId>,
    directory: String,
    attempt: Attempt,
}

impl Default for Form {
    fn default() -> Self {
        Self::on(None)
    }
}

impl Form {
    /// A blank form on `host`.
    pub const fn on(host: Option<HostId>) -> Self {
        Self {
            host,
            instance: None,
            model: None,
            effort: None,
            project: None,
            issue: None,
            directory: String::new(),
            attempt: Attempt::Editing,
        }
    }

    /// A blank form that starts a session on `issue`, on `host` unless a
    /// person picks another: continuing elsewhere is the same form.
    pub fn on_issue(host: HostId, issue: IssueId) -> Self {
        Self {
            issue: Some(issue),
            ..Self::on(Some(host))
        }
    }

    pub fn update(
        self,
        intent: FormIntent,
        hosts: &BTreeMap<HostId, Host>,
        visit: u64,
    ) -> (Self, Vec<Effect>) {
        let chosen = self.host(hosts);
        let instances =
            chosen.and_then(|(_, host)| host.fold.instances.as_ref());
        match intent {
            FormIntent::PickHost(key) => {
                let picked = hosts.keys().find(|id| id.key() == key).copied();
                (
                    Self {
                        host: picked.or(self.host),
                        instance: None,
                        model: None,
                        effort: None,
                        project: None,
                        ..self
                    },
                    Vec::new(),
                )
            }
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
                let offered = availability(instances, self.chosen(instances))
                    .map_or_else(Models::none, Availability::models);
                let picked =
                    offered.find(&ModelId(id)).map(|choice| choice.id.clone());
                (
                    Self {
                        effort: offered
                            .kept_effort(picked.as_ref(), self.effort),
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
            FormIntent::PickProject(id) => (
                Self {
                    project: (!id.is_empty()).then_some(ProjectId(id)),
                    ..self
                },
                Vec::new(),
            ),
            FormIntent::TypeDirectory(directory) => {
                (Self { directory, ..self }, Vec::new())
            }
            FormIntent::Start => {
                let Ok((host, body)) = self.start(chosen) else {
                    return (self, Vec::new());
                };
                (
                    Self {
                        attempt: Attempt::Starting(host),
                        ..self
                    },
                    Vec::from([At { host, visit }.send(body, ReplyTo::Start)]),
                )
            }
        }
    }

    fn start(
        &self,
        chosen: Option<(HostId, &Host)>,
    ) -> Result<(HostId, CommandBody), StartBlocked> {
        if matches!(self.attempt, Attempt::Starting(_)) {
            return Err(StartBlocked::Pending);
        }
        let (host, known) = chosen.ok_or(StartBlocked::Host)?;
        if !known.fold.live() {
            return Err(StartBlocked::Offline);
        }
        let instances = known.fold.instances.as_ref();
        let instance = self.chosen(instances).ok_or(StartBlocked::Instance)?;
        let models = match availability(instances, Some(instance)) {
            Some(Availability::Available { models, .. }) => models,
            Some(Availability::Unavailable { .. }) => {
                return Err(StartBlocked::UnavailableInstance);
            }
            None => return Err(StartBlocked::MissingInstance),
        };
        if self
            .model
            .as_ref()
            .is_some_and(|id| models.find(id).is_none())
        {
            return Err(StartBlocked::Model);
        }
        if models.kept_effort(self.model.as_ref(), self.effort) != self.effort {
            return Err(StartBlocked::Effort);
        }
        let at = self.place(known.fold.home.as_ref());
        if matches!(&at, StartAt::Directory(path) if path.0.is_empty()) {
            return Err(StartBlocked::Directory);
        }
        Ok((
            host,
            CommandBody::StartSession {
                instance: instance.clone(),
                at,
                model: self.model.clone(),
                effort: self.effort,
            },
        ))
    }

    pub fn forgot(&mut self, host: HostId) -> bool {
        if self.attempt == Attempt::Starting(host) {
            self.attempt = Attempt::Editing;
            true
        } else {
            false
        }
    }

    pub fn answered(self, answer: Result<CommandResult, String>) -> Self {
        Self {
            attempt: Attempt::answered(answer),
            ..self
        }
    }

    /// The default is the first live host, or the first paired.
    fn host<'a>(
        &self,
        hosts: &'a BTreeMap<HostId, Host>,
    ) -> Option<(HostId, &'a Host)> {
        if let Some(id) = self.host {
            return hosts.get(&id).map(|host| (id, host));
        }
        let listed = by_name(hosts);
        listed
            .iter()
            .find(|(_, host)| host.fold.live())
            .or_else(|| listed.first())
            .copied()
    }

    /// Where the session starts: the issue, else the project, else the
    /// directory.
    fn place(&self, home: Option<&Home>) -> StartAt {
        self.issue.map_or_else(
            || {
                self.project(home).map_or_else(
                    || {
                        StartAt::Directory(HostPath(
                            self.directory.trim().to_owned(),
                        ))
                    },
                    |project| StartAt::Project(project.clone()),
                )
            },
            StartAt::Issue,
        )
    }

    /// The project a person picked, if the host's home still lists it.
    fn project<'a>(&'a self, home: Option<&Home>) -> Option<&'a ProjectId> {
        self.project.as_ref().filter(|id| {
            home.is_some_and(|known| {
                known.projects.iter().any(|project| &project.id == *id)
            })
        })
    }

    /// The default is the first available instance, or the first listed.
    fn chosen<'a>(
        &'a self,
        instances: Option<&'a Instances>,
    ) -> Option<&'a InstanceId> {
        let known = instances.map(|listed| &listed.0);
        self.instance.as_ref().or_else(|| {
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

    pub fn screen(&self, hosts: &BTreeMap<HostId, Host>) -> NewSessionForm {
        let chosen = self.host(hosts);
        let name = chosen.map_or_default(|(_, host)| host.name());
        let instances =
            chosen.and_then(|(_, host)| host.fold.instances.as_ref());
        let empty_catalog = proto::Catalog::default();
        let catalog =
            chosen.map_or(&empty_catalog, |(_, host)| &host.fold.catalog);
        let (directory_error, instance_error, form_error) = match &self.attempt
        {
            Attempt::Refused(Refusal::Directory) => (
                Some(format!("There is no directory at that path on {name}.")),
                None,
                None,
            ),
            Attempt::Refused(Refusal::Instance(reason)) => {
                (None, Some(reason.clone()), None)
            }
            Attempt::Refused(Refusal::Other(message)) => {
                (None, None, Some(message.clone()))
            }
            Attempt::Editing | Attempt::Starting(_) => (None, None, None),
        };
        let picked = self.chosen(instances);
        let available = availability(instances, picked);
        let offered = available.map_or_else(Models::none, Availability::models);
        let suggestions = chosen
            .map_or_default(|(_, host)| host.fold.view.directories())
            .iter()
            .map(|path| Suggestion {
                name: short(path).to_owned(),
                path: path.0.clone(),
            })
            .collect::<Vec<Suggestion>>();
        let home = chosen.and_then(|(_, host)| host.fold.home.as_ref());
        let project = self.project(home);
        let issue = self.issue.map(|id| issue_title(hosts, id));
        let blocked = self.start(chosen).err();
        let start_label = if blocked == Some(StartBlocked::Pending) {
            "Starting…"
        } else {
            "Start session"
        };
        NewSessionForm {
            subtitle: String::from(if issue.is_some() {
                "Runs on the host you choose, on the issue's branch in its project there."
            } else {
                "Runs on the host you choose, in one of its projects or a directory there."
            }),
            hosts: host_choices(hosts, chosen, self.host),
            instances: instances.map_or_else(Vec::new, |listed| {
                listed
                    .0
                    .iter()
                    .map(|(id, choice)| InstanceChoice {
                        pick: FormIntent::PickInstance(id.clone()),
                        label: crate::credentials::instance_name(id, catalog),
                        state: word(Some(&choice.availability)),
                        checked: Some(id) == picked,
                    })
                    .collect()
            }),
            instance_hint: instance_hint(
                instance_error,
                Availability::note(
                    picked.and_then(|it| catalog.of(it.provider)),
                    available,
                ),
                (
                    &picked.map_or_else(
                        || String::from("That instance"),
                        |it| crate::credentials::instance_name(it, catalog),
                    ),
                    &name,
                ),
            ),
            efforts: self.effort_choices(&offered.efforts(self.model.as_ref())),
            models: self.model_choices(&offered),
            projects: home
                .filter(|_| issue.is_none())
                .map(|known| project_choices(known, project)),
            directory_shown: project.is_none() && issue.is_none(),
            issue,
            directory: self.directory.clone(),
            directory_invalid: directory_error.is_some(),
            directory_error: directory_error.map(alert),
            suggestions: (!suggestions.is_empty()).then_some(suggestions),
            error: form_error
                .or_else(|| {
                    blocked.and_then(StartBlocked::reason).map(String::from)
                })
                .map(alert),
            start_disabled: blocked.is_some(),
            start_label,
        }
    }

    /// The default first, which sends no model, then each model the host
    /// lists.
    fn model_choices(&self, offered: &Models) -> Vec<Choice> {
        let first = Choice {
            value: String::new(),
            label: offered.default_label(),
            selected: self.model.is_none(),
            disabled: false,
        };
        let listed = offered.choices.iter().map(|choice| Choice {
            value: choice.id.0.clone(),
            label: choice.name.clone(),
            selected: Some(&choice.id) == self.model.as_ref(),
            disabled: false,
        });
        let missing = self
            .model
            .as_ref()
            .filter(|id| offered.find(id).is_none())
            .map(|id| missing_choice(&id.0, "no longer offered"));
        core::iter::once(first)
            .chain(listed)
            .chain(missing)
            .collect()
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
        let missing = self
            .effort
            .filter(|effort| !offered.contains(effort))
            .map(|effort| missing_choice(effort.wire(), "no longer offered"));
        core::iter::once(first)
            .chain(listed)
            .chain(missing)
            .collect()
    }
}

fn missing_choice(value: &str, reason: &str) -> Choice {
    Choice {
        value: value.to_owned(),
        label: format!("{value} ({reason})"),
        selected: true,
        disabled: true,
    }
}

/// The issue's title as a paired host's home has it, else its id.
fn issue_title(hosts: &BTreeMap<HostId, Host>, id: IssueId) -> String {
    hosts
        .values()
        .find_map(|host| crate::issues::find(host, id))
        .map_or_else(
            || format!("{:032x}", id.0),
            |(_, issue)| issue.title.clone(),
        )
}

/// A directory first, then each project the home lists; one with no
/// checkout on the host says so and can't be picked.
fn project_choices(home: &Home, picked: Option<&ProjectId>) -> Vec<Choice> {
    let first = Choice {
        value: String::new(),
        label: String::from("A directory"),
        selected: picked.is_none(),
        disabled: false,
    };
    let listed = home.projects.iter().map(|project| Choice {
        value: project.id.0.clone(),
        label: if project.checkouts.is_empty() {
            format!("{} (not checked out)", project.title)
        } else {
            project.title.clone()
        },
        selected: Some(&project.id) == picked,
        disabled: project.checkouts.is_empty(),
    });
    core::iter::once(first).chain(listed).collect()
}

fn host_choices(
    hosts: &BTreeMap<HostId, Host>,
    chosen: Option<(HostId, &Host)>,
    selected: Option<HostId>,
) -> Vec<Choice> {
    let missing = selected
        .filter(|_| chosen.is_none())
        .map(|id| missing_choice(&id.key(), "no longer paired"));
    by_name(hosts)
        .into_iter()
        .map(|(id, host)| host_choice(id, host, chosen))
        .chain(missing)
        .collect()
}

/// A host in the host picker; one whose link isn't live says so.
fn host_choice(
    id: HostId,
    host: &Host,
    chosen: Option<(HostId, &Host)>,
) -> Choice {
    let name = host.name();
    Choice {
        value: id.key(),
        label: if host.fold.live() {
            name
        } else {
            format!("{name} (offline)")
        },
        selected: chosen.is_some_and(|(picked, _)| picked == id),
        disabled: false,
    }
}

/// A refusal of the instance, else why the host can't run it, else what
/// its provider's plugin says of it.
fn instance_hint(
    refused: Option<String>,
    note: ProviderNote,
    (label, host): (&str, &str),
) -> Option<Hint> {
    match (refused, note) {
        (Some(reason), _) | (None, ProviderNote::Unavailable(reason)) => {
            Some(alert(format!("{label} can't run on {host}: {reason}")))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartBlocked {
    Pending,
    Host,
    Offline,
    Instance,
    MissingInstance,
    UnavailableInstance,
    Model,
    Effort,
    Directory,
}

impl StartBlocked {
    const fn reason(self) -> Option<&'static str> {
        match self {
            Self::Pending | Self::UnavailableInstance => None,
            Self::Host => Some("The selected host is no longer paired."),
            Self::Offline => Some("The selected host is offline."),
            Self::Instance => Some("Choose an available instance."),
            Self::MissingInstance => {
                Some("The selected instance is no longer listed on this host.")
            }
            Self::Model => Some("The selected model is no longer offered."),
            Self::Effort => Some("The selected effort is no longer offered."),
            Self::Directory => Some("Enter a directory path."),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Attempt {
    Editing,
    Starting(HostId),
    Refused(Refusal),
}

/// Where the form shows a refusal: under the field it is about, or under
/// the whole form.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Refusal {
    Directory,
    Instance(String),
    Other(String),
}

impl Attempt {
    /// The host's answer to `StartSession`, or the reason it never arrived.
    fn answered(answer: Result<CommandResult, String>) -> Self {
        match answer {
            Ok(
                CommandResult::Accepted
                | CommandResult::Started { .. }
                | CommandResult::Instances(_)
                | CommandResult::Diff(_)
                | CommandResult::IssueCreated { .. },
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
    use proto::{CommandResult, Rejection};

    use super::{Attempt, Refusal};

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
