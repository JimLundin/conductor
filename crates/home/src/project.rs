//! A project's `projects/<project>/project.toml` in the home.
//!
//! It gives (docs/home.md §3) its name, its title, its remote, the root
//! commit that identifies its checkouts and its standards profiles. Its
//! default branch is Git's, not the home's.

use proto::ProjectId;
use serde::Deserialize;

use crate::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub title: String,
    /// `None` for a project that is local only.
    pub remote: Option<String>,
    /// The first commit of its first-parent history.
    pub root: String,
    pub profiles: Vec<String>,
}

/// The file's fields. Others are left for later steps to read.
#[derive(Deserialize)]
struct File {
    name: String,
    title: String,
    remote: Option<String>,
    root: String,
    #[serde(default)]
    profiles: Vec<String>,
}

/// The projects a home lists, by id, and why any of its folders isn't one.
/// A name is unique: a folder whose project another already has the name
/// of is a problem, and so is the other.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    pub projects: Vec<Project>,
    pub problems: Vec<String>,
}

/// Each folder of the home's `projects/` with its `project.toml`'s text, or
/// why it couldn't be read.
#[must_use]
pub fn listed(folders: Vec<(String, Result<String, String>)>) -> Listing {
    let (mut projects, mut problems) = folders.into_iter().fold(
        (Vec::new(), Vec::new()),
        |(mut projects, mut problems), (folder, text)| {
            match text.and_then(|read| parse(&folder, &read)) {
                Ok(project) => projects.push(project),
                Err(problem) => {
                    problems.push(format!("projects/{folder}: {problem}"));
                }
            }
            (projects, problems)
        },
    );
    projects.sort_by(|one, other| one.id.cmp(&other.id));
    let (unique, shared): (Vec<Project>, Vec<Project>) =
        projects.iter().cloned().partition(|project| {
            projects
                .iter()
                .filter(|other| other.name == project.name)
                .count()
                == 1
        });
    problems.extend(shared.iter().map(|project| {
        format!(
            "projects/{}: another project is also called {:?}",
            project.id.0, project.name
        )
    }));
    Listing {
        projects: unique,
        problems,
    }
}

/// The project in `folder`, as its `project.toml`'s `text` describes it.
///
/// # Errors
///
/// Why the folder's name, the file or one of its values won't do.
pub fn parse(folder: &str, text: &str) -> Result<Project, String> {
    let id = slug(folder)?;
    let file = toml::from_str::<File>(text)
        .map_err(|err| format!("project.toml: {err}"))?;
    Ok(Project {
        id,
        name: file.name,
        title: file.title,
        remote: file.remote,
        root: commit(file.root)?,
        profiles: file.profiles,
    })
}

/// A project's id is its folder's name: lower-case letters, digits and
/// dashes, so it is safe as a path and in a branch name.
fn slug(folder: &str) -> Result<ProjectId, String> {
    let fits = folder
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    (fits && !folder.is_empty() && !folder.starts_with('-'))
        .then(|| ProjectId(folder.to_owned()))
        .ok_or_else(|| {
            String::from(
                "the folder's name is not lower-case letters, digits and dashes",
            )
        })
}

/// A commit id: 40 or 64 hex digits, in lower case as Git prints it.
fn commit(root: String) -> Result<String, String> {
    let hex = root
        .chars()
        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
    (hex && matches!(root.len(), 40 | 64))
        .then_some(root)
        .ok_or_else(|| String::from("root is not a commit id"))
}
