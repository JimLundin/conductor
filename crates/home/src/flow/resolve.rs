use alloc::collections::BTreeMap;

use super::command::named;
use super::snapshot::Snapshot;
use crate::issue::{IssueReading, ReadFailure};
use crate::prelude::*;
use crate::{IssueKey, ProjectId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Cached,
    Fresh,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Missing {
    Refresh(ProjectId),
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolver {
    mode: Mode,
    refreshed: BTreeMap<ProjectId, Result<(), ReadFailure>>,
}

impl Resolver {
    #[must_use]
    pub const fn new(mode: Mode) -> Self {
        Self {
            mode,
            refreshed: BTreeMap::new(),
        }
    }

    pub fn step(
        &self,
        home: &Snapshot,
        key: &IssueKey,
        mut looked: IssueReading,
    ) -> Result<IssueReading, Missing> {
        let refreshed = self.refreshed.get(&key.project);
        let held = looked.record.is_some();
        let project = named(home, key.project);
        let id = &key.id;
        match (held, refreshed, self.mode) {
            (_, None, Mode::Fresh) | (false, None, Mode::Cached) => {
                Err(Missing::Refresh(key.project))
            }
            (true, Some(Err(failure)), _) => {
                looked.status.failure = Some(failure.clone());
                Ok(looked)
            }
            (true, _, _) => Ok(looked),
            (false, None, Mode::Offline) => Err(Missing::Unknown(format!(
                "no issue {id} in {project} is cached; `home issue list {project}` refreshes the cache"
            ))),
            (false, Some(Ok(())), _) => {
                Err(Missing::Unknown(format!("no issue {id} in {project}")))
            }
            (false, Some(Err(failure)), _) => Err(Missing::Unknown(format!(
                "no issue {id} in {project} is cached, and its tracker didn't refresh: {failure}; run `home issue list {project}` once it's reachable"
            ))),
        }
    }

    pub fn refreshed(
        &mut self,
        project: ProjectId,
        outcome: Result<(), ReadFailure>,
    ) {
        self.refreshed.insert(project, outcome);
    }
}

#[cfg(test)]
pub(crate) fn in_folder(
    home: &Snapshot,
    command: &super::command::Command,
    given: Option<&super::command::Given>,
) -> super::command::Resolved {
    let mut steps = Resolver::new(command.mode());
    command
        .issues(given)
        .into_iter()
        .map(|key| {
            loop {
                match steps.step(home, &key, home.reading(&key)) {
                    Ok(reading) => break (key, Ok(reading)),
                    Err(Missing::Unknown(reason)) => break (key, Err(reason)),
                    Err(Missing::Refresh(project)) => {
                        steps.refreshed(project, Ok(()));
                    }
                }
            }
        })
        .collect()
}
