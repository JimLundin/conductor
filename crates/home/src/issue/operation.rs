use super::{
    Applied, Edit, EditData, EditId, Issue, IssueChange, Mode, NewIssue,
    NotApplied, Observed, Part, apply, message, signer, values,
};
use crate::flow::command;
use crate::flow::role;
use crate::flow::snapshot::Snapshot;
use crate::prelude::*;
use crate::{HomeRefusal, IssueId, ProjectId};

/// The issue an operation observes before planning a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    pub project: ProjectId,
    pub issue: IssueId,
}

/// A local edit ready to observe its issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    id: EditId,
    data: EditData,
}

/// A semantic issue change and its local commit metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub target: Read,
    pub change: command::Change,
    pub marker: EditId,
    pub message: String,
    pub author: &'static str,
    pub email: &'static str,
}

/// What the decision needs from the clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoCommit {
    Unchanged,
    Dropped,
    Waiting(String),
}

/// The actions that record an edit locally and attempt to deliver it.
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

/// An edit ready to record in the clone, and what the flow warns of
/// what it writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded(Recording, Vec<String>, Option<Commit>);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Recording {
    Keep(Edit, EditId),
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

    /// The queue must be saved before writing the clone. A failed commit
    /// can be retried from the saved edit, so only persistence can reject
    /// the operation at this point.
    ///
    /// # Errors
    ///
    /// The edit could not be saved to the queue.
    pub fn next(
        &mut self,
        outcome: Result<(), String>,
    ) -> Result<Option<RecordEffect<'_>>, HomeRefusal> {
        let effect = match core::mem::replace(&mut self.0, Recording::Done) {
            Recording::Keep(edit, id) => {
                self.0 = Recording::Write(id);
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

/// Kept edits and whether the current branch covers each of them.
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
        can_commit(earlier, target.issue).then_some(target)
    }

    /// # Errors
    /// The edit is unchanged, dropped, or waiting for a valid observation.
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

    /// Save this queue before replacing the current one.
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

/// A step in delivering the pending edits to the home remote.
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
    /// Starts delivery against the fetched upstream.
    #[must_use]
    pub const fn replay(has_edits: bool) -> Self {
        if has_edits { Self::Reset } else { Self::Done }
    }

    /// The next effect, if any. An initial failed push tries the remote
    /// once more. A failed reset or replay push ends the attempt.
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

/// The facts shared by requests from the host.
pub struct Context<'a> {
    pub remote: Option<&'a str>,
    pub home: &'a Snapshot,
    pub host: &'a str,
    pub at: u64,
    pub edit: EditId,
}

impl Context<'_> {
    /// Opens issue `issue` as `new` asks, in one of the home's projects.
    ///
    /// # Errors
    ///
    /// The host has no home, or its home lists no such project.
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
        Ok(self.record(
            project,
            signer(self.host, client),
            command::Edit::New {
                id: issue,
                issue: new,
            },
        ))
    }

    /// Changes an issue as `home::flow::command` does.
    ///
    /// # Errors
    ///
    /// The host has no home, or its home lists no such issue.
    pub fn edit(
        &self,
        issue: IssueId,
        change: IssueChange,
        client: &str,
    ) -> Result<Record, HomeRefusal> {
        crate::configured(self.remote)?;
        let project = self
            .home
            .issue(issue)
            .map(|known| known.project)
            .ok_or(HomeRefusal::UnknownIssue)?;
        Ok(self.record(
            project,
            signer(self.host, client),
            command::Edit::Change {
                id: issue,
                changes: Vec::from([change]),
            },
        ))
    }

    const fn record(
        &self,
        project: ProjectId,
        by: String,
        edit: command::Edit,
    ) -> Record {
        Record {
            id: self.edit,
            data: EditData {
                project,
                at: self.at,
                by,
                edit,
                was: Vec::new(),
            },
        }
    }
}

impl Record {
    #[must_use]
    pub const fn read(&self) -> Read {
        self.data.read()
    }

    #[must_use]
    pub const fn project(&self) -> ProjectId {
        self.data.project
    }

    /// Plans the commit and retained edit against the current observation.
    ///
    /// # Errors
    /// The issue is missing, unreadable, or refused by the flow.
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
                    .map(super::Observation::issue)
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
        let next = commit.filter(|_| can_commit(&pending.0, edit.data.issue()));
        Ok(Recorded(Recording::Keep(edit, self.id), warnings, next))
    }
}

fn can_commit(earlier: &[Pending], issue: IssueId) -> bool {
    earlier
        .iter()
        .all(|pending| pending.covered || pending.edit.data.issue() != issue)
}

impl EditData {
    const fn read(&self) -> Read {
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
    pub const fn read(&self) -> Read {
        self.data.read()
    }

    /// Decides whether the edit still applies to the upstream observation, among
    /// the home's issues.
    /// # Errors
    /// The edit is unchanged, dropped, or waiting for a valid observation.
    pub fn replay(
        &self,
        home: &Snapshot,
        current: Result<Option<&Observed>, String>,
        upstream_changed: Option<u64>,
    ) -> Result<Commit, NoCommit> {
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

/// A session's updates to its issue, in the order they are attempted.
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
    /// A resumed finish uses the original edit id for its note.
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

    /// Issues the next change against the current listing. A finish
    /// consults the listing again after the note has been delivered.
    #[must_use]
    pub fn next(
        &mut self,
        home: &Snapshot,
        id: IssueId,
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

/// A Git observation needed to decide how to ready an issue branch.
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

    /// Interprets an observation and returns the next one, or the
    /// remote branch as seen by this checkout.
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
