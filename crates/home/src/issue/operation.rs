use alloc::boxed::Box;
use alloc::string::ToString;

use proto::{
    CommandId, Issue, IssueChange, IssueId, NewIssue, Part, ProjectId,
    Rejection,
};

use super::{
    Applied, Edit, EditData, Mode, apply, edit_marker, message, signer, was,
};
use crate::flow::command;
use crate::flow::role;
use crate::flow::snapshot::Snapshot;
use crate::layout;
use crate::prelude::*;

/// The issue and file an operation reads before deciding what to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    pub project: ProjectId,
    pub issue: IssueId,
    pub path: String,
}

/// A local edit ready to read its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    command: CommandId,
    data: EditData,
}

/// The files and commit a home mutation requires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub target: Read,
    pub text: String,
    pub message: String,
    pub author: &'static str,
    pub email: &'static str,
}

/// What the decision needs from the clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    Commit(Commit),
    Unchanged,
    Dropped,
    Waiting(String),
}

/// The actions that record an edit locally and attempt to deliver it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordEffect {
    Persist(Edit),
    Commit(Commit),
    Deliver {
        delivery: Delivery,
        id: Option<CommandId>,
    },
    Scan,
}

/// An edit ready to record in the clone, and what the flow warns of
/// what it writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded(Recording, Vec<String>);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Recording {
    Keep(Box<Edit>, Option<Commit>, CommandId),
    Write(CommandId, Option<Commit>),
    Push(CommandId),
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
    ) -> Result<Option<RecordEffect>, Rejection> {
        let effect = match core::mem::replace(&mut self.0, Recording::Done) {
            Recording::Keep(edit, commit, id) => {
                self.0 = Recording::Write(id, commit);
                RecordEffect::Persist(*edit)
            }
            Recording::Write(id, commit) => {
                outcome
                    .map_err(|reason| Rejection::IssueNotSaved { reason })?;
                let (next, effect) = commit.map_or(
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
    pub fn issue_for(&self, id: CommandId) -> Option<IssueId> {
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

    pub fn committed(&mut self, written: Option<CommandId>) {
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

    #[must_use]
    pub fn replay(
        &self,
        home: &Snapshot,
        index: usize,
        current: Result<Option<&str>, String>,
        upstream_changed: Option<u64>,
    ) -> Write {
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
    pub command: CommandId,
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
    ) -> Result<Record, Rejection> {
        crate::configured(self.remote)?;
        let project = self
            .home
            .projects()
            .iter()
            .find(|known| known.id == new.project)
            .map(|known| known.id)
            .ok_or(Rejection::UnknownProject)?;
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
    ) -> Result<Record, Rejection> {
        crate::configured(self.remote)?;
        let project = self
            .home
            .issue(issue)
            .map(|known| known.project)
            .ok_or(Rejection::UnknownIssue)?;
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
            command: self.command,
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
    pub fn read(&self) -> Read {
        self.data.read()
    }

    #[must_use]
    pub const fn project(&self) -> ProjectId {
        self.data.project
    }

    /// Decides the commit and what to keep and deliver, among the home's
    /// issues, the one it is to being `current`.
    ///
    /// # Errors
    ///
    /// The file is gone or cannot be read as an issue, or the flow refuses
    /// what the edit writes.
    pub fn decide(
        mut self,
        current: Option<&str>,
        home: &Snapshot,
        pending: &PendingEdits,
    ) -> Result<Recorded, Rejection> {
        self.data.was = was(home.flow().ok(), current, &self.data);
        let edit = Edit {
            command: self.command,
            data: self.data,
        };
        let (commit, warnings) = match apply(home, current, &edit, Mode::Record)
        {
            Applied::Write { text, warnings } => {
                (Some(edit.commit(text)), warnings)
            }
            Applied::AlreadyRecorded => {
                return Ok(Recorded(Recording::Done, Vec::new()));
            }
            Applied::Unchanged => (None, Vec::new()),
            Applied::Gone => return Err(Rejection::UnknownIssue),
            Applied::Unreadable(reason) => {
                return Err(Rejection::IssueNotSaved { reason });
            }
            Applied::Refused(reason) => {
                return Err(Rejection::IssueRefused { reason });
            }
        };
        let next = commit.filter(|_| can_commit(&pending.0, edit.data.issue()));
        Ok(Recorded(
            Recording::Keep(Box::new(edit), next, self.command),
            warnings,
        ))
    }
}

fn can_commit(earlier: &[Pending], issue: IssueId) -> bool {
    earlier
        .iter()
        .all(|pending| pending.covered || pending.edit.data.issue() != issue)
}

impl EditData {
    fn read(&self) -> Read {
        Read {
            project: self.project,
            issue: self.issue(),
            path: layout::issue_file(&self.project, &self.issue().to_string()),
        }
    }
}

impl Edit {
    #[must_use]
    pub fn issue_for(&self, id: CommandId) -> Option<IssueId> {
        (self.command == id).then_some(self.data.issue())
    }

    #[must_use]
    pub fn read(&self) -> Read {
        self.data.read()
    }

    /// Decides whether the edit still applies to the upstream file, among
    /// the home's issues.
    #[must_use]
    pub fn replay(
        &self,
        home: &Snapshot,
        current: Result<Option<&str>, String>,
        upstream_changed: Option<u64>,
    ) -> Write {
        let contents = match current {
            Ok(contents) => contents,
            Err(reason) => return Write::Waiting(reason),
        };
        if contents.is_none() && upstream_changed.is_some() {
            return Write::Dropped;
        }
        let changed = upstream_changed.unwrap_or(0);
        match apply(home, contents, self, Mode::Replay { changed }) {
            Applied::Write { text, .. } => Write::Commit(self.commit(text)),
            Applied::Unchanged | Applied::AlreadyRecorded => Write::Unchanged,
            Applied::Gone => Write::Dropped,
            Applied::Unreadable(reason) | Applied::Refused(reason) => {
                Write::Waiting(reason)
            }
        }
    }

    fn commit(&self, text: String) -> Commit {
        Commit {
            target: self.read(),
            text,
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
    /// A resumed finish uses the original command identity for its note.
    #[must_use]
    pub const fn command_for(
        change: &IssueChange,
        note_id: Option<CommandId>,
    ) -> Option<CommandId> {
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

/// Where a project's issue files are, and how a new one is found.
pub struct Listing {
    pub path: String,
}

impl Listing {
    #[must_use]
    pub fn creation_pattern(id: CommandId) -> String {
        format!("^{}$", crate::flow::file::comment_line(&edit_marker(id)))
    }

    #[must_use]
    pub fn created_from_path(path: &str) -> Option<IssueId> {
        layout::issue_stem(path).and_then(IssueId::parse)
    }
}
