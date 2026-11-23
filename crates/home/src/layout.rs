//! Where a home keeps its files (docs/home.md §3), known nowhere else.
//!
//! Its flow is at [`crate::flow::PATH`], and a folder of [`PROJECTS`] for
//! each project holds its [`PROJECT_FILE`] and its issues' files, one
//! `<id>.md` each, in `issues/`.
//!
//! A shell lists the folders and reads the files these name, and [`inputs`]
//! makes the home's [`Inputs`] of what it read. A shell that opens an
//! issue's file walks its path with [`IssuePath`], [`issue_parent`] and
//! [`issue_leaf`], so no link leads it out of the home.

use alloc::string::ToString;
use proto::ProjectId;

use crate::flow::graph::File;
use crate::flow::snapshot::Inputs;
use crate::prelude::*;

/// The folder whose folders are the home's projects.
pub const PROJECTS: &str = "projects";

/// The name of the file in a project's folder that describes it.
pub const PROJECT_FILE: &str = "project.toml";

/// The project in `folder` of [`PROJECTS`], from the home's root.
#[must_use]
pub fn project(folder: &str) -> String {
    format!("{PROJECTS}/{folder}")
}

/// The file that describes the project in `folder`, from the home's root.
#[must_use]
pub fn project_file(folder: &str) -> String {
    format!("{}/{PROJECT_FILE}", project(folder))
}

/// Where a project keeps its issues, from the home's root.
#[must_use]
pub fn issues(of: &ProjectId) -> String {
    format!("{}/issues", project(&of.to_string()))
}

/// An issue's file, by its name without `.md`, from the home's root.
#[must_use]
pub fn issue_file(of: &ProjectId, stem: &str) -> String {
    format!("{}/{stem}.md", issues(of))
}

/// The name without `.md` of the issue file at `path`, if it is one.
#[must_use]
pub fn issue_stem(path: &str) -> Option<&str> {
    path.rsplit('/').next()?.strip_suffix(".md")
}

/// An issue file for a shell to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueLocation {
    pub project: ProjectId,
    /// Its name without `.md`.
    pub stem: String,
    /// From the home's root.
    pub path: String,
}

/// The issue files among the `names` in the issues folder of `project`, by
/// name.
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

/// The home's inputs of what a shell read: the flow's text, each folder of
/// [`PROJECTS`] with its project file's text, and each issue file with its
/// text, or why each couldn't be read.
#[must_use]
pub fn inputs(
    flow: Result<String, String>,
    projects: Vec<(String, Result<String, String>)>,
    issues: Vec<(IssueLocation, Result<String, String>)>,
) -> Inputs {
    let (files, unread) = issues.into_iter().fold(
        (Vec::new(), Vec::new()),
        |(mut files, mut unread), (at, read)| {
            match read {
                Ok(text) => files.push(File {
                    project: at.project,
                    stem: at.stem,
                    text,
                }),
                Err(reason) => unread.push(format!("{}: {reason}", at.path)),
            }
            (files, unread)
        },
    );
    Inputs {
        flow,
        projects,
        issues: files,
        unread,
    }
}

/// A managed issue path, split into verified directory and file names.
pub struct IssuePath<T> {
    pub parents: Vec<T>,
    pub leaf: T,
}

/// A path has an unsupported component, lacks a filename, or names an unsafe leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsafeIssueEntry;

impl<T> IssuePath<T> {
    /// Separates the parents from the leaf after validating every name.
    ///
    /// # Errors
    /// The path is empty or contains a component that is not a plain name.
    pub fn checked(
        components: Option<Vec<T>>,
    ) -> Result<Self, UnsafeIssueEntry> {
        let mut parents = components.ok_or(UnsafeIssueEntry)?;
        let leaf = parents.pop().ok_or(UnsafeIssueEntry)?;
        Ok(Self { parents, leaf })
    }
}

/// How to continue after opening an issue directory without following links.
pub enum IssueParent {
    Opened,
    Absent,
    Create,
}

/// Only a write may create a missing issue directory.
#[must_use]
pub const fn issue_parent(opened: bool, create: bool) -> IssueParent {
    match (opened, create) {
        (true, _) => IssueParent::Opened,
        (false, false) => IssueParent::Absent,
        (false, true) => IssueParent::Create,
    }
}

/// Filesystem facts about an existing issue leaf.
#[derive(Clone, Copy)]
pub struct IssueLeaf {
    pub regular: bool,
    pub reparse_point: bool,
}

/// An issue leaf may be missing or an ordinary file.
///
/// # Errors
/// An occupied leaf is not an ordinary file, or is a reparse point.
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
