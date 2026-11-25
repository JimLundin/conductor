use super::{
    Applied, Edit, EditData, EditId, Issue, IssueChange, Mode, NewIssue,
    NotApplied, Observed, Part, apply, message, signer, values,
};
use crate::flow::command;
use crate::flow::role;
use crate::flow::snapshot::Snapshot;
use crate::prelude::*;
use crate::{HomeRefusal, IssueId, ProjectId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    pub project: ProjectId,
    pub issue: IssueId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    id: EditId,
    data: EditData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub target: Read,
    pub change: command::Change,
    pub marker: EditId,
    pub message: String,
    pub author: &'static str,
    pub email: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoCommit {
    Unchanged,
    Dropped,
    Waiting(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordEffect<'a> {
    Persist(Edit),
    Commit(&'a Commit),
    Deliver {
        delivery: Delivery,
        id: Option<EditId>,
    },
    Scan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded(Recording, Vec<String>, Option<Commit>);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Recording {
    Keep(Edit),
    Write(EditId),
    Push(EditId),
    Scan,
    Done,
}

impl Recorded {
    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.1
    }

    pub fn next(
        &mut self,
        outcome: Result<(), String>,
    ) -> Result<Option<RecordEffect<'_>>, HomeRefusal> {
        let effect = match core::mem::replace(&mut self.0, Recording::Done) {
            Recording::Keep(edit) => {
                self.0 = Recording::Write(edit.id);
                RecordEffect::Persist(edit)
            }
            Recording::Write(id) => {
                outcome
                    .map_err(|reason| HomeRefusal::IssueNotSaved { reason })?;
                let (next, effect) = self.2.as_ref().map_or(
                    (
                        Recording::Scan,
                        RecordEffect::Deliver {
                            delivery: Delivery::Fetch,
                            id: None,
                        },
                    ),
                    |write| (Recording::Push(id), RecordEffect::Commit(write)),
                );
                self.0 = next;
                effect
            }
            Recording::Push(id) if outcome.is_ok() => {
                self.0 = Recording::Scan;
                RecordEffect::Deliver {
                    delivery: Delivery::Push { retry: true },
                    id: Some(id),
                }
            }
            Recording::Push(_) | Recording::Scan => RecordEffect::Scan,
            Recording::Done => return Ok(None),
        };
        Ok(Some(effect))
    }
}

#[derive(Debug, Default)]
pub struct PendingEdits(Vec<Pending>);

#[derive(Debug)]
struct Pending {
    edit: Edit,
    covered: bool,
}

impl PendingEdits {
    #[must_use]
    pub fn loaded(edits: Vec<Edit>) -> Self {
        let mut pending = Self::default();
        for edit in edits {
            pending.installed(edit);
        }
        pending
    }

    #[must_use]
    pub const fn count(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn issue_for(&self, id: EditId) -> Option<IssueId> {
        self.0.iter().find_map(|pending| pending.edit.issue_for(id))
    }

    pub fn stored(&self) -> impl Iterator<Item = &Edit> {
        self.0.iter().map(|pending| &pending.edit)
    }

    pub fn with_recording<'a>(
        &'a self,
        edit: &'a Edit,
    ) -> impl Iterator<Item = &'a Edit> {
        self.stored().chain(core::iter::once(edit))
    }

    pub fn installed(&mut self, edit: Edit) {
        self.0.push(Pending {
            edit,
            covered: false,
        });
    }

    pub fn committed(&mut self, written: Option<EditId>) {
        let Some(id) = written else { return };
        if let Some(pending) = self
            .0
            .iter_mut()
            .find(|pending| pending.edit.issue_for(id).is_some())
        {
            pending.covered = true;
        }
    }

    pub fn reset(&mut self) {
        for pending in &mut self.0 {
            pending.covered = false;
        }
    }

    #[must_use]
    pub fn replay_target(&self, index: usize) -> Option<Read> {
        let (earlier, remaining) = self.0.split_at_checked(index)?;
        let target = remaining.first()?.edit.read();
        can_commit(earlier, &target.issue).then_some(target)
    }

    pub fn replay(
        &self,
        home: &Snapshot,
        index: usize,
        current: Result<Option<&Observed>, String>,
        upstream_changed: Option<u64>,
    ) -> Result<Commit, NoCommit> {
        self.0[index].edit.replay(home, current, upstream_changed)
    }

    pub fn replayed(&mut self, index: usize, covered: bool) {
        self.0[index].covered = covered;
    }

    #[must_use]
    pub fn after_push(&self) -> Option<Self> {
        if self.0.iter().all(|pending| !pending.covered) {
            return None;
        }
        Some(Self::loaded(
            self.0
                .iter()
                .filter(|pending| !pending.covered)
                .map(|pending| pending.edit.clone())
                .collect(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Push { retry: bool },
    Fetch,
    Reset,
    Rewrite,
    Landed,
    Done,
}

impl Delivery {
    #[must_use]
    pub const fn replay(has_edits: bool) -> Self {
        if has_edits { Self::Reset } else { Self::Done }
    }

    #[must_use]
    pub const fn after(self, succeeded: bool) -> Self {
        match (self, succeeded) {
            (Self::Push { .. }, true) => Self::Landed,
            (Self::Push { retry: true }, false) => Self::Fetch,
            (Self::Fetch, true) => Self::Reset,
            (Self::Reset, true) => Self::Rewrite,
            (Self::Rewrite, true) => Self::Push { retry: false },
            _ => Self::Done,
        }
    }
}

pub struct Context<'a> {
    pub remote: Option<&'a str>,
    pub home: &'a Snapshot,
    pub host: &'a str,
    pub at: u64,
    pub edit: EditId,
}

impl Context<'_> {
    pub fn open(
        &self,
        issue: IssueId,
        new: NewIssue,
        client: &str,
    ) -> Result<Record, HomeRefusal> {
        crate::configured(self.remote)?;
        let project = self
            .home
            .projects()
            .iter()
            .find(|known| known.id == new.project)
            .map(|known| known.id)
            .ok_or(HomeRefusal::UnknownProject)?;
        self.record(
            project,
            signer(self.host, client),
            command::Edit::New {
                id: issue,
                issue: new,
            },
        )
    }

    pub fn edit(
        &self,
        issue: IssueId,
        change: IssueChange,
        client: &str,
    ) -> Result<Record, HomeRefusal> {
        crate::configured(self.remote)?;
        let project = self
            .home
            .issue(&issue)
            .map(|known| known.project)
            .ok_or(HomeRefusal::UnknownIssue)?;
        self.record(
            project,
            signer(self.host, client),
            command::Edit::Change {
                id: issue,
                changes: Vec::from([change]),
            },
        )
    }

    fn record(
        &self,
        project: ProjectId,
        by: String,
        edit: command::Edit,
    ) -> Result<Record, HomeRefusal> {
        super::storage::writable(self.home, project)
            .map_err(|reason| HomeRefusal::IssueRefused { reason })?;
        Ok(Record {
            id: self.edit,
            data: EditData {
                project,
                at: self.at,
                by,
                edit,
                was: Vec::new(),
            },
        })
    }
}

impl Record {
    #[must_use]
    pub fn read(&self) -> Read {
        self.data.read()
    }

    #[must_use]
    pub const fn project(&self) -> ProjectId {
        self.data.project
    }

    pub fn decide(
        mut self,
        current: Option<&Observed>,
        home: &Snapshot,
        pending: &PendingEdits,
    ) -> Result<Recorded, HomeRefusal> {
        self.data.was = values(
            current.and_then(|observed| {
                observed
                    .reading
                    .as_ref()
                    .ok()
                    .and_then(super::Observation::issue)
            }),
            &self.data.edit,
        );
        let edit = Edit {
            id: self.id,
            data: self.data,
        };
        let (commit, warnings) = match apply(home, current, &edit, Mode::Record)
        {
            Ok(Applied { change, warnings }) => {
                (Some(edit.commit(change)), warnings)
            }
            Err(NotApplied::AlreadyRecorded) => {
                return Ok(Recorded(Recording::Done, Vec::new(), None));
            }
            Err(NotApplied::Unchanged) => (None, Vec::new()),
            Err(NotApplied::Gone) => return Err(HomeRefusal::UnknownIssue),
            Err(NotApplied::Unreadable(reason)) => {
                return Err(HomeRefusal::IssueNotSaved { reason });
            }
            Err(NotApplied::Refused(reason)) => {
                return Err(HomeRefusal::IssueRefused { reason });
            }
        };
        let next =
            commit.filter(|_| can_commit(&pending.0, &edit.data.issue()));
        Ok(Recorded(Recording::Keep(edit), warnings, next))
    }
}

fn can_commit(earlier: &[Pending], issue: &IssueId) -> bool {
    earlier
        .iter()
        .all(|pending| pending.covered || &pending.edit.data.issue() != issue)
}

impl EditData {
    fn read(&self) -> Read {
        Read {
            project: self.project,
            issue: self.issue(),
        }
    }
}

impl Edit {
    #[must_use]
    pub fn issue_for(&self, id: EditId) -> Option<IssueId> {
        (self.id == id).then_some(self.data.issue())
    }

    #[must_use]
    pub fn read(&self) -> Read {
        self.data.read()
    }

    pub fn replay(
        &self,
        home: &Snapshot,
        current: Result<Option<&Observed>, String>,
        upstream_changed: Option<u64>,
    ) -> Result<Commit, NoCommit> {
        super::storage::writable(home, self.data.project)
            .map_err(NoCommit::Waiting)?;
        let contents = match current {
            Ok(contents) => contents,
            Err(reason) => return Err(NoCommit::Waiting(reason)),
        };
        if contents.is_none() && upstream_changed.is_some() {
            return Err(NoCommit::Dropped);
        }
        let changed = upstream_changed.unwrap_or(0);
        match apply(home, contents, self, Mode::Replay { changed }) {
            Ok(Applied { change, .. }) => Ok(self.commit(change)),
            Err(NotApplied::Unchanged | NotApplied::AlreadyRecorded) => {
                Err(NoCommit::Unchanged)
            }
            Err(NotApplied::Gone) => Err(NoCommit::Dropped),
            Err(
                NotApplied::Unreadable(reason) | NotApplied::Refused(reason),
            ) => Err(NoCommit::Waiting(reason)),
        }
    }

    fn commit(&self, change: command::Change) -> Commit {
        Commit {
            target: self.read(),
            change,
            marker: self.id,
            message: message(&self.data.edit),
            author: "Conductor",
            email: "conductor@localhost",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionUpdate {
    Started,
    Done,
    Finished {
        summary: Option<String>,
        landed: u64,
    },
}

impl SessionUpdate {
    #[must_use]
    pub const fn edit_for(
        change: &IssueChange,
        note_id: Option<EditId>,
    ) -> Option<EditId> {
        match change {
            IssueChange::Note(_) => note_id,
            _ => None,
        }
    }

    #[must_use]
    pub fn next(
        &mut self,
        home: &Snapshot,
        id: &IssueId,
    ) -> Option<IssueChange> {
        let update = core::mem::replace(self, Self::Done);
        match update {
            Self::Finished {
                summary: Some(text),
                landed,
            } => {
                *self = Self::Finished {
                    summary: None,
                    landed,
                };
                Some(IssueChange::Note(text))
            }
            Self::Done => None,
            Self::Started | Self::Finished { summary: None, .. } => {
                let flow = home.flow().ok()?;
                let all: Vec<&Issue> = home.graph().issues.iter().collect();
                let issue = home.issue(id)?;
                let (moved, reason) = match update {
                    Self::Started => (
                        role::started(flow, &all, issue),
                        "a session started on it",
                    ),
                    Self::Finished { landed, .. } => (
                        (landed != 0)
                            .then(|| role::landed(flow, issue))
                            .flatten(),
                        "a session on it landed work",
                    ),
                    Self::Done => (None, ""),
                };
                Some(IssueChange::Move {
                    part: Part::Status,
                    to: String::from(moved?),
                    reason: String::from(reason),
                    signed_off: false,
                })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteProbe {
    Origin {
        branch: String,
    },
    Fetch {
        branch: String,
        refspec: String,
        tracking: String,
    },
    Tracking(String),
    Reachability {
        branch: String,
        failure: String,
    },
    Observed(super::Remote),
}

impl RemoteProbe {
    #[must_use]
    pub fn for_branch(branch: &str) -> Self {
        Self::Origin {
            branch: String::from(branch),
        }
    }

    #[must_use]
    pub fn after(self, outcome: Result<String, String>) -> Self {
        use super::Remote;

        match (self, outcome) {
            (Self::Origin { branch }, Ok(_)) => {
                let tracking =
                    branch.replacen("refs/heads/", "refs/remotes/origin/", 1);
                Self::Fetch {
                    refspec: format!("+{branch}:{tracking}"),
                    branch,
                    tracking,
                }
            }
            (Self::Fetch { tracking, .. }, Ok(_)) => Self::Tracking(tracking),
            (Self::Fetch { branch, .. }, Err(failure)) => {
                Self::Reachability { branch, failure }
            }
            (Self::Tracking(_), Ok(commit)) => {
                Self::Observed(Remote::At(commit))
            }
            (Self::Reachability { .. }, Err(reason)) => {
                Self::Observed(Remote::Unreachable(reason))
            }
            (Self::Reachability { branch, failure }, Ok(refs)) => {
                let exists = refs.lines().any(|line| {
                    line.split_once('\t')
                        .is_some_and(|(_, name)| name == branch)
                });
                Self::Observed(if exists {
                    Remote::PresentUnfetched(failure)
                } else {
                    Remote::Absent
                })
            }
            (Self::Tracking(_), Err(reason)) => {
                Self::Observed(Remote::PresentUnfetched(reason))
            }
            (Self::Observed(remote), _) => Self::Observed(remote),
            (Self::Origin { .. }, Err(_)) => Self::Observed(Remote::Absent),
        }
    }
}
