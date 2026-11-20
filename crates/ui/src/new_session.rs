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
    /// A directory first, followed by the host's projects and a retained
    /// selection if it is no longer listed.
    pub projects: Option<Vec<Choice>>,
    /// The directory field shows unless a project is picked.
    pub directory_shown: bool,
    pub directory: String,
    pub directory_invalid: bool,
    pub directory_error: Option<Hint>,
    /// The directories sessions ran in, the most recent first.
    pub suggestions: Option<Vec<Suggestion>>,
    pub refusal: Option<Hint>,
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
                let Some(picked) = data_encoding::HEXLOWER
                    .decode(key.as_bytes())
                    .ok()
                    .and_then(|bytes| bytes.try_into().ok())
                    .map(HostId)
                else {
                    return (self, Vec::new());
                };
                (
                    Self {
                        host: Some(picked),
                        instance: None,
                        model: None,
                        effort: None,
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
                let picked = (!id.is_empty()).then_some(ModelId(id));
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
                let Ok((host, instance, place)) = self.start(chosen) else {
                    return (self, Vec::new());
                };
                let submitted = Submission {
                    host,
                    instance: instance.clone(),
                    directory: match place {
                        Destination::Directory(path) => Some(path.to_owned()),
                        Destination::Project(_) | Destination::Issue(_) => None,
                    },
                };
                let body = CommandBody::StartSession {
                    instance: instance.clone(),
                    at: place.into(),
                    model: self.model.clone(),
                    effort: self.effort,
                };
                (
                    Self {
                        attempt: Attempt::Starting(submitted),
                        ..self
                    },
                    Vec::from([At { host, visit }.send(body, ReplyTo::Start)]),
                )
            }
        }
    }

    fn start<'a>(
        &'a self,
        chosen: Option<(HostId, &'a Host)>,
    ) -> Result<(HostId, &'a InstanceId, Destination<'a>), StartBlocked> {
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
        Ok((host, instance, self.place(known)?))
    }

    pub fn cancel_start_on(&mut self, host: HostId) -> bool {
        if matches!(&self.attempt, Attempt::Starting(start) if start.host == host)
        {
            self.attempt = Attempt::Editing;
            true
        } else {
            false
        }
    }

    pub fn answered(
        &mut self,
        host: &Host,
        answer: Result<CommandResult, String>,
    ) {
        if let Attempt::Starting(start) = &self.attempt {
            self.attempt = start.answered(host, answer);
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

    fn place(&self, host: &Host) -> Result<Destination<'_>, StartBlocked> {
        if self.issue.is_none() && self.project.is_none() {
            let directory = self.directory.trim();
            return if directory.is_empty() {
                Err(StartBlocked::Directory)
            } else {
                Ok(Destination::Directory(directory))
            };
        }
        let home = host.fold.home.as_ref().ok_or(StartBlocked::NoHome)?;
        let (project, destination, missing_checkout) = if let Some(issue) =
            self.issue
        {
            let (project, _) = crate::issues::find(host, issue)
                .ok_or(StartBlocked::MissingIssue)?;
            (
                project,
                Destination::Issue(issue),
                StartBlocked::IssueCheckout,
            )
        } else {
            let id =
                self.project.as_ref().ok_or(StartBlocked::MissingProject)?;
            let project = home
                .projects
                .iter()
                .find(|project| &project.id == id)
                .ok_or(StartBlocked::MissingProject)?;
            (
                project,
                Destination::Project(id),
                StartBlocked::ProjectCheckout,
            )
        };
        if project.checkouts.is_empty() {
            return Err(missing_checkout);
        }
        Ok(destination)
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
        let picked = self.chosen(instances);
        let available = availability(instances, picked);
        let label = picked.map_or_else(
            || String::from("That instance"),
            |id| crate::credentials::instance_name(id, catalog),
        );
        let directory_shown = self.project.is_none() && self.issue.is_none();
        let current_instance = instance_hint(
            Availability::note(
                picked.and_then(|it| catalog.of(it.provider)),
                available,
            ),
            (&label, &name),
        );
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
        let issue = self.issue.map(|id| issue_title(hosts, id));
        let blocked = self.start(chosen).err();
        let start_label = if blocked == Some(StartBlocked::Pending) {
            "Starting…"
        } else {
            "Start session"
        };
        let reason = blocked.and_then(StartBlocked::reason);
        let (instance_hint, directory_error, refusal) = self.start_hints(
            chosen.map(|(id, _)| (id, name.as_str())),
            picked,
            current_instance,
            reason,
        );
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
            instance_hint,
            efforts: self.effort_choices(&offered.efforts(self.model.as_ref())),
            models: self.model_choices(&offered),
            projects: (issue.is_none()
                && (home.is_some() || self.project.is_some()))
            .then(|| project_choices(home, self.project.as_ref())),
            directory_shown,
            issue,
            directory: self.directory.clone(),
            directory_invalid: directory_error.is_some(),
            directory_error,
            suggestions: (!suggestions.is_empty()).then_some(suggestions),
            refusal,
            error: reason.map(|text| alert(text.into())),
            start_disabled: blocked.is_some(),
            start_label,
        }
    }

    fn start_hints(
        &self,
        selected_host: Option<(HostId, &str)>,
        instance: Option<&InstanceId>,
        current: Option<Hint>,
        blocked: Option<&str>,
    ) -> (Option<Hint>, Option<Hint>, Option<Hint>) {
        let Attempt::Refused {
            host,
            target,
            message,
        } = &self.attempt
        else {
            return (current, None, None);
        };
        if let Some((id, name)) = selected_host
            && *host == id
        {
            match target {
                RefusalTarget::Directory(path)
                    if self.project.is_none()
                        && self.issue.is_none()
                        && self.directory.trim() == path =>
                {
                    let hint = alert(format!(
                        "There is no directory at that path on {name}."
                    ));
                    return (current, Some(hint), None);
                }
                RefusalTarget::Instance(id)
                    if instance == Some(id)
                        && current
                            .as_ref()
                            .is_none_or(|hint| hint.role.is_none()) =>
                {
                    return (Some(alert(message.clone())), None, None);
                }
                _ => {}
            }
        }
        let duplicate = blocked == Some(message.as_str())
            || current.as_ref().is_some_and(|hint| {
                hint.role.is_some() && hint.text == *message
            });
        (current, None, (!duplicate).then(|| alert(message.clone())))
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
fn project_choices(
    home: Option<&Home>,
    picked: Option<&ProjectId>,
) -> Vec<Choice> {
    let first = Choice {
        value: String::new(),
        label: String::from("A directory"),
        selected: picked.is_none(),
        disabled: false,
    };
    let projects = home.into_iter().flat_map(|known| &known.projects);
    let missing = picked
        .filter(|id| !projects.clone().any(|project| &project.id == *id))
        .map(|id| missing_choice(&id.0, "no longer listed"));
    let listed = projects.map(|project| Choice {
        value: project.id.0.clone(),
        label: if project.checkouts.is_empty() {
            format!("{} (not checked out)", project.title)
        } else {
            project.title.clone()
        },
        selected: Some(&project.id) == picked,
        disabled: project.checkouts.is_empty(),
    });
    core::iter::once(first)
        .chain(listed)
        .chain(missing)
        .collect()
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

fn instance_hint(
    note: ProviderNote,
    (label, host): (&str, &str),
) -> Option<Hint> {
    match note {
        ProviderNote::Unavailable(reason) => {
            Some(alert(format!("{label} can't run on {host}: {reason}")))
        }
        ProviderNote::Says(text) => Some(Hint {
            class: "hint muted",
            role: None,
            text,
        }),
        ProviderNote::Nothing => None,
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
    NoHome,
    MissingProject,
    ProjectCheckout,
    MissingIssue,
    IssueCheckout,
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
            Self::NoHome => Some("This host has no home configured."),
            Self::ProjectCheckout => {
                Some("The selected project is not checked out on this host.")
            }
            Self::MissingIssue => {
                Some("The selected issue is no longer listed on this host.")
            }
            Self::IssueCheckout => {
                Some("The issue's project is not checked out on this host.")
            }
            Self::MissingProject => {
                Some("The selected project is no longer listed on this host.")
            }
        }
    }
}

enum Destination<'a> {
    Directory(&'a str),
    Project(&'a ProjectId),
    Issue(IssueId),
}

impl From<Destination<'_>> for StartAt {
    fn from(destination: Destination<'_>) -> Self {
        match destination {
            Destination::Directory(path) => {
                Self::Directory(HostPath(path.to_owned()))
            }
            Destination::Project(id) => Self::Project(id.clone()),
            Destination::Issue(id) => Self::Issue(id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Attempt {
    Editing,
    Starting(Submission),
    Refused {
        host: HostId,
        target: RefusalTarget,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Submission {
    host: HostId,
    instance: InstanceId,
    directory: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RefusalTarget {
    Directory(String),
    Instance(InstanceId),
    Form,
}

impl Submission {
    fn answered(
        &self,
        host: &Host,
        answer: Result<CommandResult, String>,
    ) -> Attempt {
        let (target, message) = match answer {
            Ok(CommandResult::Rejected(Rejection::NoSuchDirectory)) => {
                let Some(path) = &self.directory else {
                    return Attempt::Refused {
                        host: self.host,
                        target: RefusalTarget::Form,
                        message: Rejection::NoSuchDirectory.label(),
                    };
                };
                (
                    RefusalTarget::Directory(path.clone()),
                    format!(
                        "There is no directory at {path} on {}.",
                        host.name()
                    ),
                )
            }
            Ok(CommandResult::Rejected(Rejection::InstanceUnavailable {
                reason,
            })) => {
                let label = crate::credentials::instance_name(
                    &self.instance,
                    &host.fold.catalog,
                );
                (
                    RefusalTarget::Instance(self.instance.clone()),
                    format!("{label} can't run on {}: {reason}", host.name()),
                )
            }
            Ok(CommandResult::Rejected(rejection)) => {
                (RefusalTarget::Form, rejection.label())
            }
            Err(error) => (RefusalTarget::Form, error),
            Ok(_) => return Attempt::Editing,
        };
        Attempt::Refused {
            host: self.host,
            target,
            message,
        }
    }
}
