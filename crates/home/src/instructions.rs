use crate::{Checkout, Project};

use crate::prelude::*;

pub const RULES: &str = "AGENTS.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sources<'a> {
    pub home: String,
    pub shared: Option<String>,
    pub own: Option<Own<'a>>,
    pub projects: &'a [Project],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Own<'a> {
    pub project: &'a Project,
    pub rules: Option<String>,
    pub cli_reads_rules: bool,
}

#[must_use]
pub fn project_of<'a>(
    projects: &'a [Project],
    directory: &str,
) -> Option<(&'a Project, &'a Checkout)> {
    projects
        .iter()
        .flat_map(|project| {
            project
                .checkouts
                .iter()
                .map(move |checkout| (project, checkout))
        })
        .filter(|(_, checkout)| holds(&checkout.path.0, directory))
        .max_by_key(|(_, checkout)| checkout.path.0.trim_end_matches('/').len())
}

fn holds(checkout: &str, directory: &str) -> bool {
    let top = checkout.trim_end_matches('/');
    directory
        .strip_prefix(top)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

#[must_use]
pub fn compose(sources: &Sources<'_>) -> Option<String> {
    let home = sources.home.trim_end_matches('/');
    let shared = sources
        .shared
        .as_deref()
        .filter(|text| !text.trim().is_empty())
        .map(|text| section("Rules for every session", text));
    let own = sources
        .own
        .as_ref()
        .map(|own| project(own, sources.projects));
    if shared.is_none() && own.is_none() {
        return None;
    }
    let mut text = format!(
        "# Instructions from the home\n\nThe owner's home repository is \
         cloned at `{home}`. Paths in these rules that name the home's \
         files, such as `docs/agents/`, are in it.\n"
    );
    text.extend(shared);
    text.extend(own);
    Some(text)
}

fn project(own: &Own<'_>, projects: &[Project]) -> String {
    let title = &own.project.title;
    let profiles = if own.project.profiles.is_empty() {
        String::from("It names no profile.\n")
    } else {
        own.project
            .profiles
            .iter()
            .map(|profile| profile_line(profile, projects))
            .collect::<Vec<String>>()
            .concat()
    };
    let rules = own
        .rules
        .as_deref()
        .filter(|text| !own.cli_reads_rules && !text.trim().is_empty())
        .map(|text| {
            section(&format!("Rules of {title} (its `AGENTS.md`)"), text)
        });
    let mut text = section(&format!("Standards of {title}"), &profiles);
    text.extend(rules);
    text
}

fn profile_line(profile: &str, projects: &[Project]) -> String {
    let provider = projects
        .iter()
        .filter(|project| project.provides.iter().any(|name| name == profile))
        .find_map(|project| project.checkouts.first());
    provider.map_or_else(
        || format!("- `{profile}`: no project checked out here provides it.\n"),
        |checkout| {
            let standards =
                format!("{}/{profile}/", checkout.path.0.trim_end_matches('/'));
            format!(
                "- `{profile}`: its checks and configs are in \
                 `{standards}`. Run `{standards}check.sh <tier>` in the \
                 session's directory.\n"
            )
        },
    )
}

fn section(heading: &str, body: &str) -> String {
    format!("\n## {heading}\n\n{}\n", body.trim_end())
}

#[cfg(test)]
mod tests;
