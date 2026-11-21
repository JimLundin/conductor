use alloc::{boxed::Box, collections::BTreeMap};

use proto::{
    CommandId, Issue, IssueChange, IssueFields, IssueId, IssueStatus,
    ProjectId, Rejection,
};

use super::{
    Applied, Change, Edit, LegacyEdit, Lines, Mode, apply, changing,
    edit_marker, file, finished, folder, listed, message, opening, parse,
    signer, started, was,
};
use crate::prelude::*;

/// The issue and file an operation reads before deciding what to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    pub issue: IssueId,
    pub path: String,
}

/// A local edit ready to read its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    command: CommandId,
    data: LegacyEdit,
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

/// An edit ready to record in the clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded(Recording);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Recording {
    Keep(Box<Edit>, Option<Commit>, CommandId),
    Write(CommandId, Option<Commit>),
    Push(CommandId),
    Scan,
    Done,
}

impl Recorded {
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

/// A step in delivering the pending edits to the home remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Push { retry: bool },
    Fetch,
    Reset,
    Rewrite,
    Persist,
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
            (Self::Rewrite, true) => Self::Persist,
            (Self::Persist, true) => Self::Push { retry: false },
            _ => Self::Done,
        }
    }
}

/// The facts shared by requests from the host.
pub struct Context<'a> {
    pub remote: Option<&'a str>,
    pub issues: &'a BTreeMap<ProjectId, Vec<Issue>>,
    pub host: &'a str,
    pub at: u64,
    pub command: CommandId,
}

impl Context<'_> {
    /// Opens an issue in a registered project.
    ///
    /// # Errors
    ///
    /// The home or project is unknown, or the fields are invalid.
    pub fn open(
        &self,
        projects: &[ProjectId],
        project: ProjectId,
        issue: IssueId,
        wanted: &IssueFields,
    ) -> Result<Record, Rejection> {
        let fields = opening(self.remote, projects, &project, wanted)?;
        Ok(Record {
            command: self.command,
            data: LegacyEdit {
                project,
                issue,
                at: self.at,
                by: String::from(self.host),
                change: Change::Create(fields),
                was: None,
            },
        })
    }

    /// Edits a field, moves the issue, changes its checklist or adds a note.
    ///
    /// # Errors
    ///
    /// The home or issue is unknown, or the change is invalid.
    pub fn edit(
        &self,
        issue: IssueId,
        wanted: IssueChange,
        client: &str,
    ) -> Result<Record, Rejection> {
        let (project, change) =
            changing(self.remote, self.issues, issue, wanted)?;
        Ok(Record {
            command: self.command,
            data: LegacyEdit {
                project,
                issue,
                at: self.at,
                by: signer(self.host, client),
                change: Change::Issue(change),
                was: None,
            },
        })
    }
}

impl Record {
    #[must_use]
    pub fn read(&self) -> Read {
        self.data.read()
    }

    /// Decides the commit and what to keep and deliver.
    ///
    /// # Errors
    ///
    /// The file is gone or cannot be read as an issue.
    pub fn decide(
        mut self,
        current: Option<&str>,
    ) -> Result<Recorded, Rejection> {
        self.data.was = was(current, self.data.issue, &self.data.change);
        let edit = Edit {
            id: Some(self.command),
            data: self.data,
        };
        let commit = match apply(current, &edit, Mode::Record) {
            Applied::Write(text) => Some(edit.commit(text)),
            Applied::AlreadyRecorded => return Ok(Recorded(Recording::Done)),
            Applied::Unchanged => None,
            Applied::Gone => return Err(Rejection::UnknownIssue),
            Applied::Unreadable(reason) => {
                return Err(Rejection::IssueNotSaved { reason });
            }
        };
        Ok(Recorded(Recording::Keep(
            Box::new(edit),
            commit,
            self.command,
        )))
    }
}

impl LegacyEdit {
    fn read(&self) -> Read {
        Read {
            issue: self.issue,
            path: file(&self.project, self.issue),
        }
    }
}

impl Edit {
    #[must_use]
    pub fn issue_for(&self, id: CommandId) -> Option<IssueId> {
        (self.id == Some(id)).then_some(self.data.issue)
    }

    #[must_use]
    pub fn read(&self) -> Read {
        self.data.read()
    }

    /// Decides whether the edit still applies to the upstream file.
    #[must_use]
    pub fn replay(
        &self,
        current: Result<Option<&str>, String>,
        changed: u64,
    ) -> Write {
        let contents = match current {
            Ok(contents) => contents,
            Err(reason) => return Write::Waiting(reason),
        };
        match apply(contents, self, Mode::Replay { changed }) {
            Applied::Write(text) => Write::Commit(self.commit(text)),
            Applied::Unchanged | Applied::AlreadyRecorded => Write::Unchanged,
            Applied::Gone => Write::Dropped,
            Applied::Unreadable(reason) => Write::Waiting(reason),
        }
    }

    fn commit(&self, text: String) -> Commit {
        Commit {
            target: self.read(),
            text,
            message: message(&self.data),
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
    /// Issues the next change against the current listing. A finish
    /// consults the listing again after the note has been delivered.
    #[must_use]
    pub fn next(
        &mut self,
        issues: &BTreeMap<ProjectId, Vec<Issue>>,
        id: IssueId,
    ) -> Option<IssueChange> {
        let update = core::mem::replace(self, Self::Done);
        let movement = match update {
            Self::Finished {
                summary: Some(text),
                landed,
            } => {
                *self = Self::Finished {
                    summary: None,
                    landed,
                };
                return Some(IssueChange::Note(text));
            }
            Self::Started => started,
            Self::Done => return None,
            Self::Finished { landed, .. } => {
                return status(issues, id)
                    .and_then(|known| finished(known, landed))
                    .map(IssueChange::Status);
            }
        };
        status(issues, id)
            .and_then(movement)
            .map(IssueChange::Status)
    }
}

fn status(
    issues: &BTreeMap<ProjectId, Vec<Issue>>,
    id: IssueId,
) -> Option<IssueStatus> {
    issues
        .values()
        .flatten()
        .find(|known| known.id == id)
        .map(|known| known.status)
}

/// A Git observation needed to decide how to ready an issue branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteProbe {
    Origin { branch: String },
    Fetch { refspec: String, tracking: String },
    Tracking(String),
    Reachability,
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
                    tracking,
                }
            }
            (Self::Fetch { tracking, .. }, Ok(_)) => Self::Tracking(tracking),
            (Self::Fetch { .. }, Err(_)) => Self::Reachability,
            (Self::Tracking(_), Ok(commit)) => {
                Self::Observed(Remote::At(commit))
            }
            (Self::Reachability, Err(reason)) => {
                Self::Observed(Remote::Unreachable(reason))
            }
            (Self::Observed(remote), _) => Self::Observed(remote),
            (
                Self::Origin { .. } | Self::Reachability | Self::Tracking(_),
                _,
            ) => Self::Observed(Remote::Absent),
        }
    }
}

/// Reads and interprets a project's issue directory.
pub struct Listing<'a> {
    project: &'a ProjectId,
    pub path: String,
}

impl<'a> Listing<'a> {
    #[must_use]
    pub fn for_project(project: &'a ProjectId) -> Self {
        Self {
            project,
            path: folder(project),
        }
    }

    #[must_use]
    pub fn opened_by(
        id: CommandId,
        files: Vec<(String, Result<String, String>)>,
    ) -> Option<IssueId> {
        let marker = edit_marker(id);
        files.into_iter().find_map(|(stem, current)| {
            let text = current.ok()?;
            let lines = Lines::of(&text).ok()?;
            lines.front.contains(&marker).then_some(())?;
            parse(&stem, &text).ok().map(|issue| issue.id)
        })
    }

    #[must_use]
    pub fn decide(
        self,
        files: Vec<(String, Result<String, String>)>,
    ) -> (Vec<Issue>, Vec<String>) {
        listed(self.project, files)
    }
}
