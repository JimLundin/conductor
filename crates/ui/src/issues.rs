//! Issues in the app (docs/home.md §7): each project's issues by status in
//! the projects view, with a field that opens one, and an issue's own pane
//! with its status, its fields, checklist and notes. The drafts the person
//! is typing are kept here until the place changes.
//!
//! The home's flow says what an issue holds: the statuses it groups by and
//! offers, in the flow's order and by its names, and the fields whose
//! values it offers. Nothing here names a status, a kind or a field: the
//! edit form's labels and the time an issue was opened are the fields with
//! those roles, and only the title is part of every issue.

use alloc::collections::BTreeMap;
use alloc::string::ToString;

use crate::prelude::*;
use proto::flow::{FieldRole, FieldType, Flow};
use proto::{
    ChecklistChange, CommandBody, Datum, Entry, Issue, IssueChange, IssueId,
    Item, NewIssue, Part, Project, ProjectId,
};

use crate::host::{Host, HostId, by_name};
use crate::{At, Effect, Intent, Place, ReplyTo};

/// Opening an issue from the projects view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewIssueIntent {
    /// The title of a new issue in this project.
    TypeTitle(ProjectId, String),
    /// The kind of a new issue in this project, by its name in the flow.
    PickKind(ProjectId, String),
    /// Opens the issue the project's title draft names, of this kind, on
    /// this host.
    Create(HostId, ProjectId, String),
}

/// Acting on the issue shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueIntent {
    /// A status's name, as the picker gives it.
    SetStatus(String),
    /// One of a field's words, as its picker gives it.
    SetField {
        field: String,
        value: String,
    },
    /// Checks, unchecks or removes an item as the person saw it.
    Checklist(ChecklistChange),
    TypeItem(String),
    AddItem,
    TypeNote(String),
    AddNote,
    /// Opens the form that edits the title, the body and the labels, when
    /// the flow has a field for them.
    Edit,
    TypeTitle(String),
    TypeBody(String),
    /// Labels, separated by commas.
    TypeLabels(String),
    Save,
    Cancel,
}

/// What the person has typed and chosen for each project's new issue, and
/// not sent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Titles {
    titles: BTreeMap<ProjectId, String>,
    kinds: BTreeMap<ProjectId, String>,
}

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
    /// Separated by commas; `None` when the issue's kind has no labels.
    labels: Option<String>,
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
    pub flow: &'issue Flow,
    pub issue: &'issue Issue,
}

impl Shown<'_> {
    /// The name of the issue's field with `role`.
    fn field(&self, role: FieldRole) -> Option<&str> {
        field_with(self.flow, self.issue, role)
    }
}

/// The name of `issue`'s field with `role`, as `flow` declares it.
fn field_with<'flow>(
    flow: &'flow Flow,
    issue: &Issue,
    role: FieldRole,
) -> Option<&'flow str> {
    flow.field_with(&issue.kind, role)
        .map(|field| field.name.as_str())
}

impl Titles {
    pub fn new_issue(
        self,
        intent: NewIssueIntent,
        visit: u64,
    ) -> (Self, Vec<Effect>) {
        match intent {
            NewIssueIntent::TypeTitle(project, title) => {
                let mut titles = self;
                titles.titles.insert(project, title);
                (titles, Vec::new())
            }
            NewIssueIntent::PickKind(project, kind) => {
                let mut titles = self;
                titles.kinds.insert(project, kind);
                (titles, Vec::new())
            }
            NewIssueIntent::Create(host, project, kind) => {
                let effects = self.create(At { host, visit }, project, kind);
                (self, effects)
            }
        }
    }

    fn create(&self, at: At, project: ProjectId, kind: String) -> Vec<Effect> {
        let title = self.titles.get(&project).map_or("", |title| title.trim());
        if title.is_empty() {
            return Vec::new();
        }
        let body = CommandBody::CreateIssue(NewIssue {
            project,
            kind,
            title: String::from(title),
            fields: Vec::new(),
        });
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
            IssueIntent::SetStatus(status) => {
                let effects = if status == issue.status {
                    Vec::new()
                } else {
                    change(IssueChange::Move {
                        part: Part::Status,
                        to: status,
                        reason: String::new(),
                    })
                };
                (self, effects)
            }
            IssueIntent::SetField { field, value } => {
                let effects = if issue.text(&field) == Some(value.as_str()) {
                    Vec::new()
                } else {
                    change(IssueChange::Set {
                        field,
                        value: Datum::Text(value),
                    })
                };
                (self, effects)
            }
            IssueIntent::Checklist(item) => {
                (self, change(IssueChange::Checklist(item)))
            }
            IssueIntent::TypeItem(item) => (Self { item, ..self }, Vec::new()),
            IssueIntent::AddItem => self.add_item(shown),
            IssueIntent::TypeNote(note) => (Self { note, ..self }, Vec::new()),
            IssueIntent::AddNote => self.add_note(shown),
            IssueIntent::Edit => {
                let fields = EditFields {
                    title: issue.title.clone(),
                    body: issue.body.clone(),
                    labels: shown
                        .field(FieldRole::Labels)
                        .map(|labels| issue.texts(labels).join(", ")),
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
                let edit = |editing: &mut EditFields| {
                    if let Some(draft) = editing.labels.as_mut() {
                        *draft = labels;
                    }
                };
                (self.edited(edit), Vec::new())
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
        let change =
            IssueChange::Checklist(ChecklistChange::Add(String::from(text)));
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
                (fields.title.trim() != original.title.trim()).then(|| {
                    IssueChange::Set {
                        field: String::from(TITLE),
                        value: Datum::Text(fields.title.clone()),
                    }
                }),
            ),
            (
                Submission::Body,
                (fields.body.trim() != original.body.trim())
                    .then(|| IssueChange::Body(fields.body.clone())),
            ),
            (Submission::Labels, labelled(shown, fields, original)),
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

/// The change that sets the labels to those `fields` give, if they differ
/// from the `original` ones and the issue's kind has a field for them.
fn labelled(
    shown: &Shown<'_>,
    fields: &EditFields,
    original: &EditFields,
) -> Option<IssueChange> {
    let field = shown.field(FieldRole::Labels)?;
    let (Some(labels), Some(before)) = (&fields.labels, &original.labels)
    else {
        return None;
    };
    (labels.trim() != before.trim()).then(|| IssueChange::Set {
        field: String::from(field),
        value: Datum::Texts(
            labels
                .split(',')
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .map(String::from)
                .collect(),
        ),
    })
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
    /// The kinds it can be, the flow's default first, the chosen one
    /// selected.
    pub kinds: Vec<Pick>,
    /// The fields of the chosen kind, in the flow's order.
    pub fields: Vec<String>,
    /// What typing the title does.
    pub project: ProjectId,
    /// Opens it, once there is a title and a live host whose home lists
    /// the project.
    pub create: Option<Intent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusGroup {
    /// Its name in the flow, which labels it and is its `data-status`.
    pub status: String,
    pub rows: Vec<IssueRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRow {
    pub key: String,
    pub title: String,
    /// The words of its fields, in the flow's order, a field it leaves out
    /// as its default, then how much of its checklist is done.
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
    let new_title = drafts.titles.get(&project.id).cloned().unwrap_or_default();
    let offered = host.and_then(|(_, known)| flow_of(known)).map(|declared| {
        (declared, chosen(declared, drafts.kinds.get(&project.id)))
    });
    let create = host
        .filter(|(_, known)| known.fold.live() && !new_title.trim().is_empty())
        .zip(offered)
        .map(|((id, _), (_, picked))| {
            Intent::NewIssue(NewIssueIntent::Create(
                id,
                project.id.clone(),
                String::from(picked),
            ))
        });
    IssueList {
        kinds: offered.map_or_else(Vec::new, |(declared, picked)| {
            kinds(declared, picked)
        }),
        fields: offered.map_or_else(Vec::new, |(declared, picked)| {
            declared
                .fields_of(picked)
                .map(|field| field.name.clone())
                .collect()
        }),
        groups: host
            .and_then(|(id, known)| Some((id, flow_of(known)?)))
            .map_or_else(Vec::new, |(id, flow)| groups(id, flow, project)),
        new_title,
        project: project.id.clone(),
        create,
    }
}

/// The kind `picked` if the flow still has it, else the flow's default.
fn chosen<'flow>(flow: &'flow Flow, picked: Option<&String>) -> &'flow str {
    picked
        .and_then(|name| flow.kind(name))
        .map_or(flow.default_kind.as_str(), |kind| kind.name.as_str())
}

/// The flow's kinds, its default first and then the rest in its order,
/// with `kind` selected.
fn kinds(flow: &Flow, kind: &str) -> Vec<Pick> {
    flow.kind(&flow.default_kind)
        .into_iter()
        .chain(
            flow.kinds
                .iter()
                .filter(|other| other.name != flow.default_kind),
        )
        .map(|offered| Pick {
            word: offered.name.clone(),
            label: offered.label.clone(),
            selected: offered.name == kind,
        })
        .collect()
}

/// The flow `host`'s home declares.
pub fn flow_of(host: &Host) -> Option<&Flow> {
    host.fold.home.as_ref()?.flow.as_deref()
}

/// What the edit form's title sets.
const TITLE: &str = "title";

/// Each status with issues, in the flow's order.
fn groups(host: HostId, flow: &Flow, project: &Project) -> Vec<StatusGroup> {
    flow.statuses
        .iter()
        .filter_map(|status| {
            let rows = project
                .issues
                .iter()
                .filter(|issue| issue.status == status.name)
                .map(|issue| IssueRow {
                    key: key_of(issue.id),
                    title: issue.title.clone(),
                    detail: detail(flow, issue),
                    open: Intent::Show(Place::Issue(host, issue.id)),
                })
                .collect::<Vec<IssueRow>>();
            (!rows.is_empty()).then(|| StatusGroup {
                status: status.name.clone(),
                rows,
            })
        })
        .collect()
}

fn detail(flow: &Flow, issue: &Issue) -> String {
    let done = issue.checklist.iter().filter(|item| item.done).count();
    let progress = (!issue.checklist.is_empty())
        .then(|| format!("{done}/{} done", issue.checklist.len()));
    flow.fields_of(&issue.kind)
        .flat_map(|field| match field.value {
            FieldType::Enum(_) => issue
                .text(&field.name)
                .or(field.default.as_deref())
                .map(String::from)
                .into_iter()
                .collect(),
            FieldType::Texts => issue.texts(&field.name).to_vec(),
            FieldType::Ref(_)
            | FieldType::Refs(_)
            | FieldType::Text
            | FieldType::Time
            | FieldType::Bool => Vec::new(),
        })
        .chain(progress)
        .collect::<Vec<String>>()
        .join(" · ")
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
    /// A picker for each field of its kind that takes one of a few words.
    pub fields: Vec<FieldPicker>,
    /// Empty when its kind has no labels.
    pub labels: String,
    pub body: String,
    /// Its kind, its branch, and when it was opened.
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
    pub word: String,
    pub label: String,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldPicker {
    /// The field's name, which also labels it.
    pub field: String,
    pub picks: Vec<Pick>,
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
    /// `None` when the issue's kind has no labels.
    pub labels: Option<String>,
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
    let flow = flow_of(host)?;
    let kind = flow
        .kind(&issue.kind)
        .map_or(issue.kind.as_str(), |kind| kind.label.as_str());
    let opened = field_with(flow, issue, FieldRole::Created)
        .and_then(|created| issue.text(created))
        .map_or_else(String::new, |created| format!(" · opened {created}"));
    Some(IssueScreen {
        key: key_of(issue.id),
        title: issue.title.clone(),
        subtitle: format!("{} · {}", project.title, host.name()),
        status: flow
            .statuses
            .iter()
            .map(|status| Pick {
                word: status.name.clone(),
                label: status.name.clone(),
                selected: status.name == issue.status,
            })
            .collect(),
        fields: pickers(flow, issue),
        labels: field_with(flow, issue, FieldRole::Labels)
            .map_or_else(String::new, |labels| issue.texts(labels).join(" · ")),
        body: issue.body.clone(),
        detail: format!("{kind} · {}{opened}", issue.branch),
        checklist: issue
            .checklist
            .iter()
            .enumerate()
            .map(|(index, item)| ItemRow {
                toggle: IssueIntent::Checklist(ChecklistChange::Check {
                    item: picked(issue, index),
                    done: !item.done,
                }),
                remove: IssueIntent::Checklist(ChecklistChange::Remove(
                    picked(issue, index),
                )),
                key: format!("{index}-{}", item.text),
                text: item.text.clone(),
                done: item.done,
            })
            .collect(),
        item: drafts.item.clone(),
        notes: issue.log.iter().map(Entry::to_string).collect(),
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

/// The issue's item at `index`, as the person sees its checklist.
fn picked(issue: &Issue, index: usize) -> Item {
    Item {
        index,
        seen: Some(issue.checklist.clone()),
    }
}

/// A picker for each field of the issue's kind whose value is one of a few
/// words, each word as the flow lists it; the field's default is picked
/// when the issue leaves it out.
fn pickers(flow: &Flow, issue: &Issue) -> Vec<FieldPicker> {
    flow.fields_of(&issue.kind)
        .filter_map(|field| {
            let FieldType::Enum(words) = &field.value else {
                return None;
            };
            let current = issue.text(&field.name).or(field.default.as_deref());
            Some(FieldPicker {
                field: field.name.clone(),
                picks: words
                    .iter()
                    .map(|word| Pick {
                        word: word.clone(),
                        label: word.clone(),
                        selected: current == Some(word.as_str()),
                    })
                    .collect(),
            })
        })
        .collect()
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
