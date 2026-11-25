use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use serde::{Deserialize, Serialize};

use crate::issue::{EditId, IssueChange, Part};
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

    /// # Errors
    /// The operation belongs to another host.
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
    /// Fingerprint of the relevant observed preconditions.
    pub expected: String,
    pub work: Work,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Work {
    ChangeIssue { issue: IssueId, change: IssueChange },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Step {
    ApplyChange,
    RemoveTransitionValue(Part),
    SetTransitionValue(Part),
    SetOpenState,
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

    /// # Errors
    /// The caller reused the id with a different intent or is offline before a new attempt.
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

    /// # Errors
    /// The precondition differs from the last verified intermediate state, or a prior dispatch is unresolved.
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

    /// # Errors
    /// Only the currently attempted step can be confirmed.
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
