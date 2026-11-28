use alloc::collections::BTreeMap;
use alloc::format;

use crate::log::Event;
use crate::registry::InstanceId;
use crate::session::{EventKind, SessionId};
use agents::{Account, ClientName};
use home::IssueKey;

use crate::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub issue: IssueKey,
    pub(crate) instance: InstanceId,
}

impl Assignment {
    #[must_use]
    pub fn signer(&self) -> ClientName {
        let provider = self.instance.provider.name();
        ClientName(match &self.instance.account {
            Account::Own => String::from(provider),
            Account::Named(account) => format!("{provider} · {account}"),
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Assigned {
    pub(crate) issue: Option<IssueKey>,
    pub(crate) instance: Option<InstanceId>,
}

impl Assigned {
    #[must_use]
    pub(crate) fn after(self, kind: &EventKind) -> Self {
        match kind {
            EventKind::SessionCreated { instance, .. } => Self {
                instance: Some(instance.clone()),
                ..self
            },
            EventKind::OnIssue { issue } => Self {
                issue: Some(issue.clone()),
                ..self
            },
            EventKind::SessionRemoved { .. } => Self {
                issue: None,
                ..self
            },
            _ => self,
        }
    }

    #[must_use]
    pub(crate) fn is(&self, issue: IssueKey, instance: &InstanceId) -> bool {
        self.issue == Some(issue) && self.instance.as_ref() == Some(instance)
    }
}

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
