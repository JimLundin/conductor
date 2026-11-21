use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use proto::{CommandResult, Credential, InstanceId, Rejection, Roster};

use super::Retry;
use crate::fleet::Desired;
use crate::serve::Configure;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Work {
    Install(InstanceId, Credential),
    Remove(InstanceId),
}

impl Work {
    const fn instance(&self) -> &InstanceId {
        match self {
            Self::Install(id, _) | Self::Remove(id) => id,
        }
    }

    #[must_use]
    pub fn command(&self) -> Configure {
        match self {
            Self::Install(id, credential) => Configure::SetCredential {
                provider: id.provider,
                credential: credential.clone(),
            },
            Self::Remove(id) => Configure::Remove(id.clone()),
        }
    }
}

#[derive(Default)]
pub struct Reconciliation {
    desired: Option<Desired>,
    revision: Option<proto::fleet::Revision>,
    adopted: Roster,
    owned: BTreeSet<InstanceId>,
    failures: BTreeMap<InstanceId, (Work, Option<Retry>)>,
}

impl Reconciliation {
    #[must_use]
    pub fn new(revision: Option<proto::fleet::Revision>) -> Self {
        Self {
            revision,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn changed_providers(&self) -> BTreeSet<proto::Provider> {
        self.desired.as_ref().map_or_else(BTreeSet::new, |desired| {
            crate::fleet::changed_providers(
                self.revision.as_ref(),
                &desired.revision,
            )
        })
    }

    pub fn reconfigured(&mut self) {
        self.revision = self
            .desired
            .as_ref()
            .map(|desired| desired.revision.clone());
    }

    pub fn desire(&mut self, desired: Desired) {
        if self
            .desired
            .as_ref()
            .is_some_and(|current| current.version > desired.version)
        {
            return;
        }
        self.owned.extend(desired.owned.iter().cloned());
        self.desired = Some(desired);
        let pending = self.pending();
        self.failures.retain(|_, (work, _)| pending.contains(work));
    }

    #[must_use]
    pub fn plan(&self, now: u64) -> Vec<Work> {
        self.pending()
            .into_iter()
            .filter(|work| {
                self.failures.get(work.instance()).is_none_or(|(_, retry)| {
                    retry.as_ref().is_some_and(|scheduled| scheduled.at <= now)
                })
            })
            .collect()
    }

    fn pending(&self) -> Vec<Work> {
        let Some(desired) = &self.desired else {
            return Vec::new();
        };
        let mut work: Vec<_> = self
            .owned
            .iter()
            .filter(|id| desired.credentials.of(id).is_none())
            .cloned()
            .map(Work::Remove)
            .collect();
        for (id, settings) in &desired.credentials.0 {
            if self.adopted.of(id) != Some(settings)
                && let Some(credential) = &settings.credential
            {
                work.push(Work::Install(id.clone(), credential.clone()));
            }
        }
        work
    }

    pub fn completed(&mut self, work: Work, result: &CommandResult, now: u64) {
        if !self.pending().contains(&work) {
            return;
        }
        let id = work.instance().clone();
        if crate::fleet::applied(&work.command(), result) {
            self.failures.remove(&id);
            match work {
                Work::Remove(_) => {
                    self.adopted.0.remove(&id);
                    self.owned.remove(&id);
                }
                Work::Install(_, _) => {
                    if let Some(settings) = self
                        .desired
                        .as_ref()
                        .and_then(|desired| desired.credentials.of(&id))
                    {
                        self.adopted.0.insert(id.clone(), settings.clone());
                        self.owned.insert(id);
                    }
                }
            }
        } else {
            let retry = if matches!(
                result,
                CommandResult::Rejected(
                    Rejection::InvalidCredential { .. }
                        | Rejection::UnknownInstance
                )
            ) {
                None
            } else {
                let attempt = self
                    .failures
                    .get(&id)
                    .and_then(|(_, retry)| retry.as_ref())
                    .map_or(0, |retry| retry.attempt.saturating_add(1));
                Some(Retry::after(now, attempt))
            };
            self.failures.insert(id, (work, retry));
        }
    }

    #[must_use]
    pub fn settled(&self) -> bool {
        self.pending().is_empty()
    }

    #[must_use]
    pub const fn owned(&self) -> &BTreeSet<InstanceId> {
        &self.owned
    }
}
