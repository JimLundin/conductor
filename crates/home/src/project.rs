use crate::ProjectId;
use alloc::string::ToString;
use serde::Deserialize;

use crate::layout::{self, PROJECT_FILE};
use crate::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub title: String,
    pub remote: Option<String>,
    pub root: String,
    pub profiles: Vec<String>,
    pub provides: Vec<String>,
    pub backends: crate::backend::Backends,
}

#[derive(Deserialize)]
struct File {
    name: String,
    title: String,
    remote: Option<String>,
    root: String,
    #[serde(default)]
    profiles: Vec<String>,
    #[serde(default)]
    provides: Vec<String>,
    #[serde(default)]
    backends: crate::backend::Backends,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    pub projects: Vec<Project>,
    pub problems: Vec<String>,
}

#[must_use]
pub fn listed(folders: Vec<(String, Result<String, String>)>) -> Listing {
    let (mut projects, mut problems) = folders.into_iter().fold(
        (Vec::new(), Vec::new()),
        |(mut projects, mut problems), (folder, text)| {
            match text.and_then(|read| parse(&folder, &read)) {
                Ok(project) => projects.push(project),
                Err(problem) => {
                    problems.push(format!(
                        "{}: {problem}",
                        layout::project(&folder)
                    ));
                }
            }
            (projects, problems)
        },
    );
    projects.sort_by_key(|project| project.id);
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
            "{}: another project is also called {:?}",
            layout::project(&project.id.to_string()),
            project.name
        )
    }));
    Listing {
        projects: unique,
        problems,
    }
}

pub fn parse(folder: &str, text: &str) -> Result<Project, String> {
    let id = slug(folder)?;
    let file = toml::from_str::<File>(text)
        .map_err(|err| format!("{PROJECT_FILE}: {err}"))?;
    file.backends
        .validate()
        .map_err(|problem| format!("{PROJECT_FILE}: {problem:?}"))?;
    Ok(Project {
        id,
        name: file.name,
        title: file.title,
        remote: file.remote,
        root: commit(file.root)?,
        profiles: file.profiles,
        provides: file.provides,
        backends: file.backends,
    })
}

fn slug(folder: &str) -> Result<ProjectId, String> {
    ProjectId::parse(folder).ok_or_else(|| {
        String::from(
            "the folder's name is not a project id of 6 base 32 digits",
        )
    })
}

fn commit(root: String) -> Result<String, String> {
    let hex = root
        .chars()
        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
    (hex && matches!(root.len(), 40 | 64))
        .then_some(root)
        .ok_or_else(|| String::from("root is not a commit id"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn invalid_backend_references_are_refused_when_reading_project_configuration()
     {
        let text = r#"
name = "local"
title = "Local"
root = "fc27e50efb661d7ef4a45d3edbe0234ff0a6fbd5"
[backends.tracker]
adapter = ""
resource = "issues"
"#;
        assert!(super::parse("a1b2c3", text).is_err());
    }
}
