use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use serde::{Deserialize, Serialize};

use crate::issue::{
    Comment, CommentRequest, CommentWrite, EditId, IssueCreate, IssueWrite,
    Lookup, NativeIssue, Opened,
};
use crate::{IssueId, ProjectId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handle {
    pub owner: [u8; 32],
    pub id: EditId,
}

impl Handle {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let (encoded, id) = text.split_once('/')?;
        let owner = data_encoding::HEXLOWER.decode(encoded.as_bytes()).ok()?;
        Some(Self {
            owner: owner.try_into().ok()?,
            id: EditId::parse(id)?,
        })
    }

    #[must_use]
    pub fn marker(self) -> String {
        alloc::format!("conductor-operation {self}")
    }

    pub fn local(self, host: [u8; 32]) -> Result<(), Refusal> {
        (self.owner == host)
            .then_some(())
            .ok_or(Refusal::OwnerUnavailable)
    }
}

impl fmt::Display for Handle {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            out,
            "{}/{}",
            data_encoding::HEXLOWER.encode(&self.owner),
            self.id
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Intent {
    pub project: ProjectId,
    pub expected: String,
    pub work: Work,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Work {
    ChangeIssue {
        issue: IssueId,
        writes: Vec<IssueWrite>,
    },
    Comment {
        issue: IssueId,
        write: CommentWrite,
    },
    Create {
        create: IssueCreate,
    },
}

impl Intent {
    #[must_use]
    pub fn edit(issue: &Lookup, seen: String, writes: Vec<IssueWrite>) -> Self {
        Self {
            project: issue.project,
            expected: seen,
            work: Work::ChangeIssue {
                issue: issue.id.clone(),
                writes,
            },
        }
    }

    pub fn unchanged(
        seen: &str,
        observed: &NativeIssue,
    ) -> Result<(), Refusal> {
        (observed.revision() == seen)
            .then_some(())
            .ok_or(Refusal::Conflict)
    }

    pub fn edits(
        &self,
        issue: &Lookup,
        seen: &str,
    ) -> Result<&[IssueWrite], Refusal> {
        match &self.work {
            Work::ChangeIssue { issue: id, writes }
                if self.project == issue.project
                    && *id == issue.id
                    && self.expected == seen =>
            {
                Ok(writes)
            }
            Work::ChangeIssue { .. } => Err(Refusal::ChangedIntent),
            Work::Comment { .. } | Work::Create { .. } => {
                Err(Refusal::NotAnEdit)
            }
        }
    }

    pub fn comment(
        handle: Handle,
        request: &CommentRequest,
    ) -> Result<Self, Refusal> {
        if !request.write.body().contains(&handle.marker()) {
            return Err(Refusal::Unmarked);
        }
        Ok(Self {
            project: request.issue.project,
            expected: request.seen.clone(),
            work: Work::Comment {
                issue: request.issue.id.clone(),
                write: request.write.clone(),
            },
        })
    }
}

impl Intent {
    pub fn create(
        handle: Handle,
        project: ProjectId,
        create: IssueCreate,
    ) -> Result<Self, Refusal> {
        if !create.body.contains(&handle.to_string()) {
            return Err(Refusal::Unmarked);
        }
        Ok(Self {
            project,
            expected: String::new(),
            work: Work::Create { create },
        })
    }

    #[must_use]
    pub fn opens(&self, project: ProjectId, create: &IssueCreate) -> bool {
        matches!(&self.work, Work::Create { create: held }
            if self.project == project
                && held.title == create.title
                && held.labels == create.labels)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opening {
    Send(IssueCreate),
    Settled(IssueId),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Advance {
    Send(IssueWrite),
    Done,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    Send,
    Settled(Comment),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Step {
    WriteIssue(usize),
    WriteComment,
    CreateIssue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Confirmed {
    pub step: Step,
    pub revision: String,
    pub identity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Progress {
    Ready,
    AwaitingEvidence(Step),
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub handle: Handle,
    pub intent: Intent,
    confirmed: Vec<Confirmed>,
    progress: Progress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("the operation belongs to another host")]
    OwnerUnavailable,
    #[error("the operation id was already bound to a different intent")]
    ChangedIntent,
    #[error("an online observation is required")]
    Offline,
    #[error("the observed preconditions have changed")]
    Conflict,
    #[error("the attempted step requires verified evidence")]
    AwaitingEvidence,
    #[error("this step was already confirmed")]
    AlreadyConfirmed,
    #[error("the operation has already completed")]
    Completed,
    #[error("the operation has no attempted step to confirm")]
    NoAttempt,
    #[error("the write's body lacks its operation marker")]
    Unmarked,
    #[error("the operation writes no comment")]
    NotAComment,
    #[error("the operation edits no issue")]
    NotAnEdit,
    #[error("the operation opens no issue")]
    NotACreate,
}

impl Record {
    #[must_use]
    pub const fn new(handle: Handle, intent: Intent) -> Self {
        Self {
            handle,
            intent,
            confirmed: Vec::new(),
            progress: Progress::Ready,
        }
    }

    pub fn resume(
        handle: Handle,
        intent: Intent,
        stored: Option<Self>,
        online: bool,
    ) -> Result<Self, Refusal> {
        match stored {
            Some(record)
                if record.handle != handle || record.intent != intent =>
            {
                Err(Refusal::ChangedIntent)
            }
            Some(record) => Ok(record),
            None if online => Ok(Self::new(handle, intent)),
            None => Err(Refusal::Offline),
        }
    }

    #[must_use]
    pub fn confirmed(&self) -> &[Confirmed] {
        &self.confirmed
    }

    #[must_use]
    pub const fn progress(&self) -> &Progress {
        &self.progress
    }

    pub fn dispatch(
        &self,
        step: Step,
        observed: &str,
        online: bool,
    ) -> Result<Self, Refusal> {
        match &self.progress {
            Progress::Completed => return Err(Refusal::Completed),
            Progress::AwaitingEvidence(_) => {
                return Err(Refusal::AwaitingEvidence);
            }
            Progress::Ready => {}
        }
        if !online {
            return Err(Refusal::Offline);
        }
        if self
            .confirmed
            .iter()
            .any(|confirmed| confirmed.step == step)
        {
            return Err(Refusal::AlreadyConfirmed);
        }
        let expected = self
            .confirmed
            .last()
            .map_or(self.intent.expected.as_str(), |last| {
                last.revision.as_str()
            });
        if observed != expected {
            return Err(Refusal::Conflict);
        }
        Ok(Self {
            progress: Progress::AwaitingEvidence(step),
            ..self.clone()
        })
    }

    pub fn confirm(
        &self,
        evidence: Confirmed,
        complete: bool,
    ) -> Result<Self, Refusal> {
        match &self.progress {
            Progress::AwaitingEvidence(step) if *step == evidence.step => {}
            Progress::AwaitingEvidence(_) => {
                return Err(Refusal::AwaitingEvidence);
            }
            Progress::Completed => return Err(Refusal::Completed),
            Progress::Ready => return Err(Refusal::NoAttempt),
        }
        let mut record = self.clone();
        record.confirmed.push(evidence);
        record.progress = if complete {
            Progress::Completed
        } else {
            Progress::Ready
        };
        Ok(record)
    }
}

impl Record {
    #[must_use]
    pub fn awaited(&self) -> Option<CommentRequest> {
        let (Progress::AwaitingEvidence(_), Work::Comment { issue, write }) =
            (&self.progress, &self.intent.work)
        else {
            return None;
        };
        Some(CommentRequest {
            issue: Lookup {
                project: self.intent.project,
                id: issue.clone(),
            },
            write: write.clone(),
            seen: self.intent.expected.clone(),
        })
    }

    pub fn reject(&self) -> Result<Self, Refusal> {
        let Progress::AwaitingEvidence(_) = self.progress else {
            return Err(Refusal::NoAttempt);
        };
        Ok(Self {
            progress: Progress::Ready,
            ..self.clone()
        })
    }

    pub fn next(&self, observed: &[Comment]) -> Result<(Self, Next), Refusal> {
        let Work::Comment { write, .. } = &self.intent.work else {
            return Err(Refusal::NotAComment);
        };
        let marker = self.handle.marker();
        let mut carrying = observed
            .iter()
            .filter(|comment| write.covers(comment))
            .filter(|comment| comment.body.contains(&marker));
        match (&self.progress, carrying.next(), carrying.next()) {
            (Progress::Completed, Some(found), None) => {
                Ok((self.clone(), Next::Settled(found.clone())))
            }
            (Progress::AwaitingEvidence(step), Some(found), None) => {
                let evidence = Confirmed {
                    step: step.clone(),
                    revision: write.precondition(core::slice::from_ref(found)),
                    identity: Some(found.id.clone()),
                };
                Ok((
                    self.confirm(evidence, true)?,
                    Next::Settled(found.clone()),
                ))
            }
            (Progress::Ready, None, None) => {
                self.send(write, observed).map(|sent| (sent, Next::Send))
            }
            _ => Ok((self.clone(), Next::Unknown)),
        }
    }

    pub fn opened(&self, found: &[Opened]) -> Result<(Self, Opening), Refusal> {
        let Work::Create { create } = &self.intent.work else {
            return Err(Refusal::NotACreate);
        };
        let mut carrying = found
            .iter()
            .filter(|issue| issue.native.title == create.title);
        match (&self.progress, carrying.next(), carrying.next()) {
            (Progress::Completed, Some(found), None) => {
                Ok((self.clone(), Opening::Settled(found.id.clone())))
            }
            (Progress::AwaitingEvidence(step), Some(found), None) => {
                let evidence = Confirmed {
                    step: step.clone(),
                    revision: found.native.revision(),
                    identity: Some(found.id.to_string()),
                };
                Ok((
                    self.confirm(evidence, true)?,
                    Opening::Settled(found.id.clone()),
                ))
            }
            (Progress::Ready, None, None) => {
                let sent = self.dispatch(Step::CreateIssue, "", true)?;
                Ok((sent, Opening::Send(create.clone())))
            }
            _ => Ok((self.clone(), Opening::Unknown)),
        }
    }

    pub fn advance(
        &self,
        observed: &NativeIssue,
        resuming: bool,
    ) -> Result<(Self, Advance), Refusal> {
        let Work::ChangeIssue { writes, .. } = &self.intent.work else {
            return Err(Refusal::NotAnEdit);
        };
        let mut record = self.clone();
        loop {
            let done = record.confirmed.len();
            let before = record
                .confirmed
                .last()
                .map_or(&record.intent.expected, |last| &last.revision)
                .clone();
            let Some(write) = writes.get(done) else {
                return Ok((record, Advance::Done));
            };
            record = match record.progress.clone() {
                Progress::Completed => return Ok((record, Advance::Done)),
                Progress::Ready => {
                    let step = Step::WriteIssue(done);
                    let sent =
                        record.dispatch(step, &observed.revision(), true)?;
                    return Ok((sent, Advance::Send(write.clone())));
                }
                Progress::AwaitingEvidence(step) => {
                    if write.holds(observed)
                        && write.undone(observed).revision() == before
                    {
                        let evidence = Confirmed {
                            step,
                            revision: observed.revision(),
                            identity: None,
                        };
                        record.confirm(evidence, done + 1 == writes.len())?
                    } else if resuming && observed.revision() == before {
                        record.reject()?
                    } else {
                        return Ok((record, Advance::Unknown));
                    }
                }
            };
        }
    }

    fn send(
        &self,
        write: &CommentWrite,
        observed: &[Comment],
    ) -> Result<Self, Refusal> {
        if !observed.iter().any(|comment| write.covers(comment))
            && matches!(write, CommentWrite::Edit { .. })
        {
            return Err(Refusal::Conflict);
        }
        self.dispatch(Step::WriteComment, &write.precondition(observed), true)
    }
}

#[cfg(test)]
mod tests;

#[must_use]
pub fn status_query(
    args: &[String],
) -> Option<Result<(Handle, Option<&str>), &'static str>> {
    let [project, command, rest @ ..] = args else {
        return None;
    };
    if project != "project" || command != "operation-status" {
        return None;
    }
    let (encoded, directory) = match rest {
        [handle] => (handle, None),
        [handle, flag, dir] if flag == "--data-dir" => {
            (handle, Some(dir.as_str()))
        }
        _ => {
            return Some(Err(
                "usage: home project operation-status <handle> [--data-dir <dir>]",
            ));
        }
    };
    Some(
        Handle::parse(encoded)
            .map(|handle| (handle, directory))
            .ok_or("invalid operation handle"),
    )
}
