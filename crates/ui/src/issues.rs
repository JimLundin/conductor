//! Issues in the app (docs/home.md §7): each project's issues by status in
//! the projects view, with a field that opens one, and an issue's own pane
//! with its status, priority, checklist and notes. The drafts the person
//! is typing are kept here until the place changes.

use alloc::collections::BTreeMap;

use crate::prelude::*;
use proto::{
    CommandBody, Issue, IssueChange, IssueFields, IssueId, IssueStatus,
    Priority, Project, ProjectId,
};

use crate::host::{Host, HostId, by_name};
use crate::{At, Effect, Intent, Place, ReplyTo};

/// Opening an issue from the projects view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewIssueIntent {
    /// The title of a new issue in this project.
    TypeTitle(ProjectId, String),
    /// Opens the issue the project's title draft names, on this host.
    Create(HostId, ProjectId),
}

/// Acting on the issue shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueIntent {
    /// A status's word, as the picker gives it.
    SetStatus(String),
    /// A priority's word, as the picker gives it.
    SetPriority(String),
    /// The checklist's item at this index, checked or unchecked.
    Toggle(usize),
    RemoveItem(usize),
    TypeItem(String),
    AddItem,
    TypeNote(String),
    AddNote,
    /// Opens the form that edits the title, the body and the labels.
    Edit,
    TypeTitle(String),
    TypeBody(String),
    /// Labels, separated by commas.
    TypeLabels(String),
    Save,
    Cancel,
}

/// What the person has typed and not sent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Titles(BTreeMap<ProjectId, String>);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IssueDraft {
    item: String,
    note: String,
    editing: Option<Editing>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Editing {
    title: String,
    body: String,
    labels: String,
}

/// The issue shown, and where commands about it go.
pub struct Shown<'issue> {
    pub at: At,
    pub issue: &'issue Issue,
}

impl Titles {
    pub fn new_issue(
        &mut self,
        intent: NewIssueIntent,
        visit: u64,
    ) -> Vec<Effect> {
        match intent {
            NewIssueIntent::TypeTitle(project, title) => {
                self.0.insert(project, title);
                Vec::new()
            }
            NewIssueIntent::Create(host, project) => {
                self.create(At { host, visit }, project)
            }
        }
    }

    fn create(&self, at: At, project: ProjectId) -> Vec<Effect> {
        let title = self.0.get(&project).map_or("", |title| title.trim());
        if title.is_empty() {
            return Vec::new();
        }
        let body = CommandBody::CreateIssue {
            fields: IssueFields {
                title: String::from(title),
                body: String::new(),
                priority: Priority::Normal,
                labels: Vec::new(),
            },
            project,
        };
        Vec::from([at.send(body, ReplyTo::Created)])
    }
}

impl IssueDraft {
    pub fn update(
        &mut self,
        intent: IssueIntent,
        shown: &Shown<'_>,
    ) -> Vec<Effect> {
        let issue = shown.issue;
        let change = match intent {
            IssueIntent::SetStatus(word) => IssueStatus::named(&word)
                .filter(|status| *status != issue.status)
                .map(IssueChange::Status),
            IssueIntent::SetPriority(word) => Priority::named(&word)
                .filter(|priority| *priority != issue.priority)
                .map(IssueChange::Priority),
            IssueIntent::Toggle(index) => {
                issue.checklist.get(index).map(|item| IssueChange::Check {
                    item: item.text.clone(),
                    done: !item.done,
                })
            }
            IssueIntent::RemoveItem(index) => issue
                .checklist
                .get(index)
                .map(|item| IssueChange::RemoveItem(item.text.clone())),
            IssueIntent::AddItem => {
                take_trimmed(&mut self.item).map(IssueChange::AddItem)
            }
            IssueIntent::AddNote => {
                take_trimmed(&mut self.note).map(IssueChange::Note)
            }
            IssueIntent::Save => return self.save(shown),
            IssueIntent::TypeItem(item) => {
                self.item = item;
                None
            }
            IssueIntent::TypeNote(note) => {
                self.note = note;
                None
            }
            IssueIntent::Edit => {
                self.editing = Some(Editing {
                    title: issue.title.clone(),
                    body: issue.body.clone(),
                    labels: issue.labels.join(", "),
                });
                None
            }
            IssueIntent::Cancel => {
                self.editing = None;
                None
            }
            IssueIntent::TypeTitle(title) => {
                if let Some(form) = &mut self.editing {
                    form.title = title;
                }
                None
            }
            IssueIntent::TypeBody(body) => {
                if let Some(form) = &mut self.editing {
                    form.body = body;
                }
                None
            }
            IssueIntent::TypeLabels(labels) => {
                if let Some(form) = &mut self.editing {
                    form.labels = labels;
                }
                None
            }
        };
        change.into_iter().map(|edit| send(shown, edit)).collect()
    }

    /// Sends each field the form changed, its own change.
    fn save(&mut self, shown: &Shown<'_>) -> Vec<Effect> {
        let Some(editing) = self.editing.take_if(|editing| editing.saves())
        else {
            return Vec::new();
        };
        let issue = shown.issue;
        let labels = editing
            .labels
            .split(',')
            .map(|label| String::from(label.trim()))
            .filter(|label| !label.is_empty())
            .collect::<Vec<String>>();
        let changes = [
            (editing.title.trim() != issue.title)
                .then_some(IssueChange::Title(editing.title)),
            (editing.body.trim() != issue.body)
                .then_some(IssueChange::Body(editing.body)),
            (labels != issue.labels).then_some(IssueChange::Labels(labels)),
        ];
        changes
            .into_iter()
            .flatten()
            .map(|change| send(shown, change))
            .collect()
    }
}

fn take_trimmed(draft: &mut String) -> Option<String> {
    let text = draft.trim();
    if text.is_empty() {
        return None;
    }
    let value = String::from(text);
    draft.clear();
    Some(value)
}

impl Editing {
    /// An issue keeps a title.
    fn saves(&self) -> bool {
        !self.title.trim().is_empty()
    }
}

const fn send(shown: &Shown<'_>, change: IssueChange) -> Effect {
    shown.at.send(
        CommandBody::ChangeIssue {
            issue: shown.issue.id,
            change,
        },
        ReplyTo::Notice,
    )
}

/// A project's issues, as the projects view lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueList {
    /// Each status with issues, in the order of the lifecycle.
    pub groups: Vec<StatusGroup>,
    /// The new issue's title so far.
    pub new_title: String,
    /// What typing the title does.
    pub project: ProjectId,
    /// Opens it, once there is a title and a live host whose home lists
    /// the project.
    pub create: Option<Intent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusGroup {
    /// Also its `data-status`.
    pub status: &'static str,
    pub label: &'static str,
    pub rows: Vec<IssueRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRow {
    pub key: String,
    pub title: String,
    /// Its priority, then its labels and how much of its checklist is done.
    pub detail: String,
    pub open: Intent,
}

/// The issues of `project` on `host`, the first live host whose home lists
/// it, else the first that does.
pub fn list(
    project: &Project,
    host: Option<(HostId, &Host)>,
    drafts: &Titles,
) -> IssueList {
    let new_title = drafts.0.get(&project.id).cloned().unwrap_or_default();
    let create = host
        .filter(|(_, known)| known.fold.live() && !new_title.trim().is_empty())
        .map(|(id, _)| {
            Intent::NewIssue(NewIssueIntent::Create(id, project.id.clone()))
        });
    IssueList {
        groups: host.map_or_else(Vec::new, |(id, _)| groups(id, project)),
        new_title,
        project: project.id.clone(),
        create,
    }
}

fn groups(host: HostId, project: &Project) -> Vec<StatusGroup> {
    IssueStatus::ALL
        .into_iter()
        .filter_map(|status| {
            let rows = project
                .issues
                .iter()
                .filter(|issue| issue.status == status)
                .map(|issue| IssueRow {
                    key: key_of(issue.id),
                    title: issue.title.clone(),
                    detail: detail(issue),
                    open: Intent::Show(Place::Issue(host, issue.id)),
                })
                .collect::<Vec<IssueRow>>();
            (!rows.is_empty()).then(|| StatusGroup {
                status: status.word(),
                label: label(status),
                rows,
            })
        })
        .collect()
}

fn detail(issue: &Issue) -> String {
    let done = issue.checklist.iter().filter(|item| item.done).count();
    let progress = (!issue.checklist.is_empty())
        .then(|| format!("{done}/{} done", issue.checklist.len()));
    core::iter::once(String::from(issue.priority.word()))
        .chain(issue.labels.iter().cloned())
        .chain(progress)
        .collect::<Vec<String>>()
        .join(" · ")
}

const fn label(status: IssueStatus) -> &'static str {
    match status {
        IssueStatus::Open => "Open",
        IssueStatus::Ready => "Ready",
        IssueStatus::InProgress => "In progress",
        IssueStatus::Review => "Review",
        IssueStatus::Done => "Done",
        IssueStatus::Dropped => "Dropped",
    }
}

/// An issue's id in hex, which keys it in the page.
fn key_of(id: IssueId) -> String {
    format!("{:032x}", id.0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueScreen {
    pub key: String,
    pub title: String,
    /// Its project, and the host whose home it is read from.
    pub subtitle: String,
    pub status: Vec<Pick>,
    pub priority: Vec<Pick>,
    pub labels: String,
    pub body: String,
    /// Its branch, and when it was opened.
    pub detail: String,
    pub checklist: Vec<ItemRow>,
    pub item: String,
    pub notes: Vec<String>,
    pub note: String,
    pub editing: Option<EditForm>,
    /// The host is live, so the issue can change.
    pub enabled: bool,
    pub notice: Option<String>,
    /// Opens the new-session form on the issue.
    pub start: Intent,
    /// The sessions on it, on every paired host.
    pub sessions: Vec<IssueSession>,
}

/// A session on the issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueSession {
    pub key: String,
    /// Its instance and host, and its state.
    pub label: String,
    pub open: Intent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub word: &'static str,
    pub label: &'static str,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRow {
    pub index: usize,
    pub key: String,
    pub text: String,
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditForm {
    pub title: String,
    pub body: String,
    pub labels: String,
    pub save_disabled: bool,
}

/// The issue `id` as `host`'s home has it, if it still does.
pub fn find(host: &Host, id: IssueId) -> Option<(&Project, &Issue)> {
    host.fold
        .home
        .as_ref()?
        .projects
        .iter()
        .find_map(|project| {
            project
                .issues
                .iter()
                .find(|issue| issue.id == id)
                .map(|issue| (project, issue))
        })
}

pub fn screen(
    (at, host): (HostId, &Host),
    id: IssueId,
    (drafts, hosts): (&IssueDraft, &BTreeMap<HostId, Host>),
    notice: Option<String>,
) -> Option<IssueScreen> {
    let (project, issue) = find(host, id)?;
    Some(IssueScreen {
        key: key_of(issue.id),
        title: issue.title.clone(),
        subtitle: format!("{} · {}", project.title, host.name()),
        status: IssueStatus::ALL
            .into_iter()
            .map(|status| Pick {
                word: status.word(),
                label: label(status),
                selected: status == issue.status,
            })
            .collect(),
        priority: Priority::ALL
            .into_iter()
            .map(|priority| Pick {
                word: priority.word(),
                label: priority.word(),
                selected: priority == issue.priority,
            })
            .collect(),
        labels: issue.labels.join(" · "),
        body: issue.body.clone(),
        detail: format!("{} · opened {}", issue.branch, issue.created),
        checklist: issue
            .checklist
            .iter()
            .enumerate()
            .map(|(index, item)| ItemRow {
                index,
                key: format!("{index}-{}", item.text),
                text: item.text.clone(),
                done: item.done,
            })
            .collect(),
        item: drafts.item.clone(),
        notes: issue.notes.clone(),
        note: drafts.note.clone(),
        editing: drafts.editing.as_ref().map(|editing| EditForm {
            title: editing.title.clone(),
            body: editing.body.clone(),
            labels: editing.labels.clone(),
            save_disabled: !editing.saves(),
        }),
        enabled: host.fold.live(),
        notice,
        start: Intent::StartOn(at, id),
        sessions: sessions(hosts, id),
    })
}

/// The sessions on issue `id` on every paired host, by host and the most
/// recently active first.
fn sessions(hosts: &BTreeMap<HostId, Host>, id: IssueId) -> Vec<IssueSession> {
    by_name(hosts)
        .into_iter()
        .flat_map(|(host_id, host)| {
            let catalog = &host.fold.catalog;
            host.fold
                .view
                .recent()
                .into_iter()
                .filter_map(|session| {
                    host.fold
                        .view
                        .sessions
                        .get(&session)
                        .map(|view| (session, view))
                })
                .filter(|(_, view)| view.issue == Some(id))
                .map(|(session, view)| IssueSession {
                    key: format!("{}-{:032x}", host_id.key(), session.0),
                    label: format!(
                        "{} on {} · {}",
                        crate::credentials::instance_name(
                            &view.instance,
                            catalog
                        ),
                        host.name(),
                        crate::session::status_text(view.status_label())
                    ),
                    open: Intent::Show(Place::Session(host_id, session)),
                })
                .collect::<Vec<IssueSession>>()
        })
        .collect()
}
