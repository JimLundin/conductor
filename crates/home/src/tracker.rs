use crate::ProjectId;
use crate::flow::snapshot::Snapshot;
use crate::prelude::*;
use crate::project::Project;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tracker<'a> {
    Markdown,
    Configured(&'a Project),
}

#[must_use]
pub const fn of(project: &Project) -> Tracker<'_> {
    if project.backends.tracker.is_some() {
        Tracker::Configured(project)
    } else {
        Tracker::Markdown
    }
}

#[must_use]
pub fn select(home: &Snapshot, project: ProjectId) -> Option<Tracker<'_>> {
    home.projects()
        .iter()
        .find(|known| known.id == project)
        .map(of)
}

pub fn writable(home: &Snapshot, project: ProjectId) -> Result<(), String> {
    match select(home, project) {
        Some(Tracker::Markdown) => Ok(()),
        Some(Tracker::Configured(_)) => Err(String::from(
            "edit this issue in the configured tracker; local issue writes are disabled for this project",
        )),
        None => Err(String::from(
            "local issue writes require a valid project configuration",
        )),
    }
}
