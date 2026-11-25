use crate::ProjectId;
use alloc::string::ToString;

use crate::issue::storage::File;
use crate::issue::storage::Inputs;
use crate::prelude::*;

pub const PROJECTS: &str = "projects";

pub const PROJECT_FILE: &str = "project.toml";

#[must_use]
pub fn project(folder: &str) -> String {
    format!("{PROJECTS}/{folder}")
}

#[must_use]
pub fn project_file(folder: &str) -> String {
    format!("{}/{PROJECT_FILE}", project(folder))
}

#[must_use]
pub fn issues(of: &ProjectId) -> String {
    format!("{}/issues", project(&of.to_string()))
}

#[must_use]
pub fn issue_file(of: &ProjectId, stem: &str) -> String {
    format!("{}/{stem}.md", issues(of))
}

#[must_use]
pub fn issue_stem(path: &str) -> Option<&str> {
    path.rsplit('/').next()?.strip_suffix(".md")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueLocation {
    pub project: ProjectId,
    pub stem: String,
    pub path: String,
}

#[must_use]
pub fn issue_files(
    of: &ProjectId,
    mut names: Vec<String>,
) -> Vec<IssueLocation> {
    names.sort();
    names
        .iter()
        .filter_map(|name| issue_stem(name))
        .map(|stem| IssueLocation {
            project: *of,
            stem: String::from(stem),
            path: issue_file(of, stem),
        })
        .collect()
}

#[must_use]
pub fn inputs(
    flow: Result<String, String>,
    projects: Vec<(String, Result<String, String>)>,
    issues: Vec<(IssueLocation, Result<String, String>)>,
) -> Inputs {
    let (files, unavailable, unread) = issues.into_iter().fold(
        (Vec::new(), Vec::new(), Vec::new()),
        |(mut files, mut unavailable, mut unread), (at, read)| {
            match read {
                Ok(text) => files.push(File {
                    project: at.project,
                    stem: at.stem,
                    text,
                }),
                Err(reason) => {
                    if let Some(id) = crate::IssueId::parse(&at.stem) {
                        unavailable.push(crate::issue::Unreadable {
                            id,
                            project: at.project,
                            title: None,
                            body: None,
                            created: None,
                            reason,
                        });
                    } else {
                        unread.push(format!("{}: {reason}", at.path));
                    }
                }
            }
            (files, unavailable, unread)
        },
    );
    Inputs {
        flow,
        projects,
        issues: files,
        unavailable,
        observed_at: None,
        unread,
    }
}

pub struct IssuePath<T> {
    pub parents: Vec<T>,
    pub leaf: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsafeIssueEntry;

impl<T> IssuePath<T> {
    pub fn checked(
        components: Option<Vec<T>>,
    ) -> Result<Self, UnsafeIssueEntry> {
        let mut parents = components.ok_or(UnsafeIssueEntry)?;
        let leaf = parents.pop().ok_or(UnsafeIssueEntry)?;
        Ok(Self { parents, leaf })
    }
}

pub enum IssueParent {
    Opened,
    Absent,
    Create,
}

#[must_use]
pub const fn issue_parent(opened: bool, create: bool) -> IssueParent {
    match (opened, create) {
        (true, _) => IssueParent::Opened,
        (false, false) => IssueParent::Absent,
        (false, true) => IssueParent::Create,
    }
}

#[derive(Clone, Copy)]
pub struct IssueLeaf {
    pub regular: bool,
    pub reparse_point: bool,
}

pub const fn issue_leaf(
    file: Option<IssueLeaf>,
) -> Result<(), UnsafeIssueEntry> {
    match file {
        None
        | Some(IssueLeaf {
            regular: true,
            reparse_point: false,
        }) => Ok(()),
        Some(_) => Err(UnsafeIssueEntry),
    }
}

#[cfg(test)]
mod tests;
