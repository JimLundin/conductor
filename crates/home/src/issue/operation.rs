use alloc::{boxed::Box, collections::BTreeMap};
use sha2::{Digest, Sha256};

use proto::{
    CommandId, Issue, IssueChange, IssueFields, IssueId, IssueStatus,
    ProjectId, Rejection,
};

use super::{
    Applied, Change, Edit, EditData, EditIdentity, Mode, apply, changing, date,
    edit_marker, file, finished, folder, listed, message, name, opening, parse,
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
        index: usize,
        current: Result<Option<&str>, String>,
        upstream_changed: Option<u64>,
    ) -> Write {
        self.0[index].edit.replay(current, upstream_changed)
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
            data: EditData {
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
            data: EditData {
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
        pending: &PendingEdits,
    ) -> Result<Recorded, Rejection> {
        self.data.was = was(current, self.data.issue, &self.data.change);
        let edit = Edit {
            identity: EditIdentity::Command(self.command),
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
        let next = commit.filter(|_| can_commit(&pending.0, edit.data.issue));
        Ok(Recorded(Recording::Keep(
            Box::new(edit),
            next,
            self.command,
        )))
    }
}

fn can_commit(earlier: &[Pending], issue: IssueId) -> bool {
    earlier
        .iter()
        .all(|pending| pending.covered || pending.edit.data.issue != issue)
}

impl EditData {
    pub(super) fn rendered_note(&self, text: &str) -> String {
        let body = text.trim().lines().collect::<Vec<_>>().join("\n");
        format!("{} {}: {body}", date(self.at), self.by)
    }

    fn read(&self) -> Read {
        Read {
            issue: self.issue,
            path: file(&self.project, self.issue),
        }
    }
}

impl Edit {
    #[must_use]
    pub fn legacy_note_target(&self) -> Option<Read> {
        self.legacy_note().map(|_| self.read())
    }

    fn legacy_note(&self) -> Option<&str> {
        match (&self.identity, &self.data.change) {
            (EditIdentity::Legacy, Change::Issue(IssueChange::Note(text))) => {
                Some(text)
            }
            _ => None,
        }
    }

    fn legacy_note_key(&self) -> Option<(IssueId, String)> {
        self.legacy_note()
            .map(|text| (self.data.issue, self.data.rendered_note(text)))
    }

    /// A legacy queue starts after the notes already in its upstream
    /// baseline. Local commits may contain only a prefix of the queue.
    pub fn migrate_legacy_notes(
        edits: &mut [Self],
        upstream: &BTreeMap<IssueId, String>,
    ) {
        let mut occurrences = BTreeMap::new();
        for (&id, text) in upstream {
            if let Ok(issue) = parse(&name(id), text) {
                for note in issue.notes {
                    *occurrences.entry((id, note)).or_insert(0) += 1;
                }
            }
        }
        let mut queued = BTreeMap::new();
        for edit in edits {
            if let Some(key) = edit.legacy_note_key() {
                let ordinal = occurrences.entry(key.clone()).or_insert(0);
                *ordinal += 1;
                let (issue, note) = key;
                let digest = Sha256::digest(note.as_bytes());
                let hash = data_encoding::HEXLOWER.encode(&digest);
                let nth =
                    queued.entry((issue, note, edit.data.at)).or_insert(0);
                *nth += 1;
                edit.identity = EditIdentity::MigratedNote {
                    ordinal: *ordinal,
                    marker: format!(
                        "# conductor-legacy-note: {hash} {} {nth}",
                        edit.data.at
                    ),
                };
            }
        }
    }

    #[must_use]
    pub fn issue_for(&self, id: CommandId) -> Option<IssueId> {
        (self.identity == EditIdentity::Command(id)).then_some(self.data.issue)
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
        issues: &BTreeMap<ProjectId, Vec<Issue>>,
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
                let from = status(issues, id)?;
                let to = match update {
                    Self::Started => started(from),
                    Self::Finished { landed, .. } => finished(from, landed),
                    Self::Done => None,
                }?;
                Some(IssueChange::Transition { from, to })
            }
        }
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
    pub fn creation_pattern(id: CommandId) -> String {
        format!("^{}$", edit_marker(id))
    }

    #[must_use]
    pub fn created_from_path(path: &str) -> Option<IssueId> {
        let stem = path.rsplit('/').next()?.strip_suffix(".md")?;
        ulid::Ulid::from_string(stem).ok().map(|id| IssueId(id.0))
    }

    #[must_use]
    pub fn decide(
        self,
        files: Vec<(String, Result<String, String>)>,
    ) -> (Vec<Issue>, Vec<String>) {
        listed(self.project, files)
    }
}
