//! Issues in the app (docs/home.md §7): each project's issues by status in
//! the projects view, with a field that opens one, and an issue's own pane
//! with its status, priority, checklist and notes. The drafts the person
//! is typing are kept here until the place changes.

use alloc::collections::BTreeMap;

use crate::prelude::*;
use proto::{
    ChecklistAction, ChecklistItem, CommandBody, Issue, IssueChange,
    IssueFields, IssueId, IssueStatus, Priority, Project, ProjectId,
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
    Checklist {
        snapshot: Vec<ChecklistItem>,
        index: usize,
        action: ChecklistAction,
    },
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
    item_sent: Option<String>,
    note_sent: Option<String>,
    saving: Option<Save>,
    errors: Vec<String>,
    editing: Option<Editing>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Editing {
    fields: EditFields,
    original: EditFields,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EditFields {
    title: String,
    body: String,
    labels: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Save {
    fields: EditFields,
    pending: usize,
    failed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Submission {
    Item,
    Note,
    Title,
    Body,
    Labels,
}

impl Submission {
    const fn label(self) -> &'static str {
        match self {
            Self::Item => "Checklist item",
            Self::Note => "Note",
            Self::Title => "Title",
            Self::Body => "Body",
            Self::Labels => "Labels",
        }
    }
}

/// The issue shown, and where commands about it go.
pub struct Shown<'issue> {
    pub at: At,
    pub issue: &'issue Issue,
}

impl Titles {
    pub fn new_issue(
        self,
        intent: NewIssueIntent,
        visit: u64,
    ) -> (Self, Vec<Effect>) {
        match intent {
            NewIssueIntent::TypeTitle(project, title) => {
                let mut titles = self.0;
                titles.insert(project, title);
                (Self(titles), Vec::new())
            }
            NewIssueIntent::Create(host, project) => {
                let effects = self.create(At { host, visit }, project);
                (self, effects)
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
        self,
        intent: IssueIntent,
        shown: &Shown<'_>,
    ) -> (Self, Vec<Effect>) {
        let issue = shown.issue;
        let change = |change| send(shown, change);
        match intent {
            IssueIntent::SetStatus(word) => (
                self,
                IssueStatus::named(&word)
                    .filter(|status| *status != issue.status)
                    .map_or_else(Vec::new, |status| {
                        change(IssueChange::Status(status))
                    }),
            ),
            IssueIntent::SetPriority(word) => (
                self,
                Priority::named(&word)
                    .filter(|priority| *priority != issue.priority)
                    .map_or_else(Vec::new, |priority| {
                        change(IssueChange::Priority(priority))
                    }),
            ),
            IssueIntent::Checklist {
                snapshot,
                index,
                action,
            } => (
                self,
                change(IssueChange::Checklist {
                    snapshot,
                    index,
                    action,
                }),
            ),
            IssueIntent::TypeItem(item) => (Self { item, ..self }, Vec::new()),
            IssueIntent::AddItem => self.add_item(shown),
            IssueIntent::TypeNote(note) => (Self { note, ..self }, Vec::new()),
            IssueIntent::AddNote => self.add_note(shown),
            IssueIntent::Edit => {
                let fields = EditFields {
                    title: issue.title.clone(),
                    body: issue.body.clone(),
                    labels: issue.labels.join(", "),
                };
                (
                    Self {
                        editing: Some(Editing {
                            original: fields.clone(),
                            fields,
                        }),
                        ..self
                    },
                    Vec::new(),
                )
            }
            IssueIntent::TypeTitle(title) => {
                (self.edited(|editing| editing.title = title), Vec::new())
            }
            IssueIntent::TypeBody(body) => {
                (self.edited(|editing| editing.body = body), Vec::new())
            }
            IssueIntent::TypeLabels(labels) => {
                (self.edited(|editing| editing.labels = labels), Vec::new())
            }
            IssueIntent::Save => self.save(shown),
            IssueIntent::Cancel if self.saving.is_some() => (self, Vec::new()),
            IssueIntent::Cancel => (
                Self {
                    editing: None,
                    ..self
                },
                Vec::new(),
            ),
        }
    }

    fn add_item(mut self, shown: &Shown<'_>) -> (Self, Vec<Effect>) {
        let text = self.item.trim();
        if text.is_empty() || self.item_sent.is_some() {
            return (self, Vec::new());
        }
        let change = IssueChange::AddItem(String::from(text));
        self.item_sent = Some(self.item.clone());
        (self, submit(shown, change, Submission::Item))
    }

    fn add_note(mut self, shown: &Shown<'_>) -> (Self, Vec<Effect>) {
        let text = self.note.trim();
        if text.is_empty() || self.note_sent.is_some() {
            return (self, Vec::new());
        }
        let change = IssueChange::Note(String::from(text));
        self.note_sent = Some(self.note.clone());
        (self, submit(shown, change, Submission::Note))
    }

    fn edited(mut self, change: impl FnOnce(&mut EditFields)) -> Self {
        if let Some(open) = self.editing.as_mut() {
            change(&mut open.fields);
        }
        self
    }

    fn save(mut self, shown: &Shown<'_>) -> (Self, Vec<Effect>) {
        let Some(editing) = self
            .editing
            .as_ref()
            .filter(|editing| self.saving.is_none() && editing.saves())
        else {
            return (self, Vec::new());
        };
        let fields = &editing.fields;
        let original = &editing.original;
        let changes = [
            (
                Submission::Title,
                (fields.title.trim() != original.title.trim())
                    .then(|| IssueChange::Title(fields.title.clone())),
            ),
            (
                Submission::Body,
                (fields.body.trim() != original.body.trim())
                    .then(|| IssueChange::Body(fields.body.clone())),
            ),
            (
                Submission::Labels,
                (fields.labels.trim() != original.labels.trim()).then(|| {
                    IssueChange::Labels(
                        fields
                            .labels
                            .split(',')
                            .map(str::trim)
                            .filter(|label| !label.is_empty())
                            .map(String::from)
                            .collect(),
                    )
                }),
            ),
        ];
        let effects: Vec<_> = changes
            .into_iter()
            .filter_map(|(field, modified)| {
                modified
                    .map(|change| effect(shown, change, ReplyTo::Issue(field)))
            })
            .collect();
        if effects.is_empty() {
            self.editing = None;
        } else {
            self.saving = Some(Save {
                fields: fields.clone(),
                pending: effects.len(),
                failed: false,
            });
        }
        (self, effects)
    }

    pub(crate) fn answered(
        &mut self,
        field: Submission,
        error: Option<String>,
    ) {
        let prefix = format!("{}: ", field.label());
        let failed = error.is_some();
        self.errors.retain(|kept| !kept.starts_with(&prefix));
        self.errors
            .extend(error.map(|message| format!("{prefix}{message}")));
        match field {
            Submission::Item => {
                settle_text(&mut self.item_sent, &mut self.item, failed);
            }
            Submission::Note => {
                settle_text(&mut self.note_sent, &mut self.note, failed);
            }
            Submission::Title | Submission::Body | Submission::Labels => {
                self.saved_field(field, failed);
            }
        }
    }

    fn saved_field(&mut self, field: Submission, failed: bool) {
        let (Some(save), Some(editing)) = (&mut self.saving, &mut self.editing)
        else {
            return;
        };
        if !failed {
            match field {
                Submission::Title => {
                    editing.original.title.clone_from(&save.fields.title);
                }
                Submission::Body => {
                    editing.original.body.clone_from(&save.fields.body);
                }
                Submission::Labels => {
                    editing.original.labels.clone_from(&save.fields.labels);
                }
                Submission::Item | Submission::Note => {}
            }
        }
        save.failed |= failed;
        save.pending -= 1;
        if save.pending == 0 {
            if !save.failed && editing.fields == save.fields {
                self.editing = None;
            }
            self.saving = None;
        }
    }
}

fn settle_text(sent: &mut Option<String>, draft: &mut String, failed: bool) {
    if sent.take().as_ref() == Some(draft) && !failed {
        draft.clear();
    }
}

impl Editing {
    fn saves(&self) -> bool {
        !self.fields.title.trim().is_empty()
    }
}

fn submit(
    shown: &Shown<'_>,
    change: IssueChange,
    field: Submission,
) -> Vec<Effect> {
    Vec::from([effect(shown, change, ReplyTo::Issue(field))])
}

fn send(shown: &Shown<'_>, change: IssueChange) -> Vec<Effect> {
    Vec::from([effect(shown, change, ReplyTo::Notice)])
}

const fn effect(
    shown: &Shown<'_>,
    change: IssueChange,
    reply: ReplyTo,
) -> Effect {
    shown.at.send(
        CommandBody::ChangeIssue {
            issue: shown.issue.id,
            change,
        },
        reply,
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
    pub item_pending: bool,
    pub note_pending: bool,
    pub submission_errors: Vec<String>,
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
    pub toggle: IssueIntent,
    pub remove: IssueIntent,
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
                toggle: IssueIntent::Checklist {
                    snapshot: issue.checklist.clone(),
                    index,
                    action: ChecklistAction::SetDone(!item.done),
                },
                remove: IssueIntent::Checklist {
                    snapshot: issue.checklist.clone(),
                    index,
                    action: ChecklistAction::Remove,
                },
                key: format!("{index}-{}", item.text),
                text: item.text.clone(),
                done: item.done,
            })
            .collect(),
        item: drafts.item.clone(),
        notes: issue.notes.clone(),
        note: drafts.note.clone(),
        item_pending: drafts.item_sent.is_some(),
        note_pending: drafts.note_sent.is_some(),
        submission_errors: drafts.errors.clone(),
        editing: drafts.editing.as_ref().map(|editing| EditForm {
            title: editing.fields.title.clone(),
            body: editing.fields.body.clone(),
            labels: editing.fields.labels.clone(),
            save_disabled: !editing.saves() || drafts.saving.is_some(),
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
                .filter(|(session, view)| {
                    view.issue == Some(id)
                        && crate::movement::visible(
                            hosts, host_id, *session, view,
                        )
                })
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
