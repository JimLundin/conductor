//! The issue a session works on and the instance whose agent works it, as
//! the session's log has them (docs/home.md §4).

use alloc::collections::BTreeMap;
use alloc::format;

use proto::{
    Account, ClientName, Event, EventKind, InstanceId, IssueId, SessionId,
};

use crate::prelude::*;

/// A session on an issue and its instance, which signs the notes the
/// session writes on the issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub issue: IssueId,
    pub(crate) instance: InstanceId,
}

impl Assignment {
    /// Who signs the session's notes on its issue: its instance.
    #[must_use]
    pub fn signer(&self) -> ClientName {
        let provider = self.instance.provider.name();
        ClientName(match &self.instance.account {
            Account::Own => String::from(provider),
            Account::Named(account) => format!("{provider} · {account}"),
        })
    }
}

/// The issue and provider instance a session owns on this host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Assigned {
    pub(crate) issue: Option<IssueId>,
    pub(crate) instance: Option<InstanceId>,
}

impl Assigned {
    /// The issue and instance after `kind`.
    #[must_use]
    pub(crate) fn after(self, kind: &EventKind) -> Self {
        match kind {
            EventKind::SessionCreated { instance, .. } => Self {
                instance: Some(instance.clone()),
                ..self
            },
            EventKind::OnIssue { issue } => Self {
                issue: Some(*issue),
                ..self
            },
            EventKind::SessionRemoved { .. }
            | EventKind::SessionMoved { .. } => Self {
                issue: None,
                ..self
            },
            _ => self,
        }
    }

    /// Whether the session is on `issue` in `instance`.
    #[must_use]
    pub(crate) fn is(&self, issue: IssueId, instance: &InstanceId) -> bool {
        self.issue == Some(issue) && self.instance.as_ref() == Some(instance)
    }

    /// The assignment, once both the issue and the instance are known.
    #[must_use]
    pub(crate) fn assignment(self) -> Option<Assignment> {
        Some(Assignment {
            issue: self.issue?,
            instance: self.instance?,
        })
    }
}

/// Each session's issue and instance after `history`.
#[must_use]
pub(crate) fn assigned(history: &[Event]) -> BTreeMap<SessionId, Assigned> {
    let mut sessions = BTreeMap::<SessionId, Assigned>::new();
    for event in history {
        let session = sessions.entry(event.session).or_default();
        *session = core::mem::take(session).after(&event.kind);
    }
    sessions
}

#[cfg(test)]
mod tests;
