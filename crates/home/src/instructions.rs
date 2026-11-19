//! What a session is told when it starts (docs/home.md §5).
//!
//! That is the home's shared rules, where its project's profiles keep their
//! standards on this host, and the project's own rules when the session's
//! CLI doesn't read them itself.

use proto::{Checkout, Project};

use crate::prelude::*;

/// The file that holds rules: the home's shared ones at its top, and a
/// project's own in its checkout.
pub const RULES: &str = "AGENTS.md";

/// What the host read for a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sources<'a> {
    /// Where the host keeps its clone of the home.
    pub home: String,
    /// The home's `AGENTS.md`.
    pub shared: Option<String>,
    /// The project the session is in, if it is in one.
    pub own: Option<Own<'a>>,
}

/// The project a session is in, as its directory has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Own<'a> {
    pub project: &'a Project,
    /// The project's [`RULES`].
    pub rules: Option<String>,
    /// The directory holds the file the session's CLI reads by itself,
    /// which then tells it the project's rules.
    pub cli_reads_rules: bool,
}

/// The project with a checkout at `directory` or around it, and that
/// checkout.
#[must_use]
pub fn project_of<'a>(
    projects: &'a [Project],
    directory: &str,
) -> Option<(&'a Project, &'a Checkout)> {
    projects.iter().find_map(|project| {
        project
            .checkouts
            .iter()
            .find(|checkout| holds(&checkout.path.0, directory))
            .map(|checkout| (project, checkout))
    })
}

fn holds(checkout: &str, directory: &str) -> bool {
    let top = checkout.trim_end_matches('/');
    directory
        .strip_prefix(top)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// The session's instructions, or `None` when there is nothing to tell.
#[must_use]
pub fn compose(sources: &Sources<'_>) -> Option<String> {
    let home = sources.home.trim_end_matches('/');
    let shared = sources
        .shared
        .as_deref()
        .filter(|text| !text.trim().is_empty())
        .map(|text| section("Rules for every session", text));
    let own = sources.own.as_ref().map(|own| project(home, own));
    if shared.is_none() && own.is_none() {
        return None;
    }
    let mut text = format!(
        "# Instructions from the home\n\nThe owner's home repository is \
         cloned at `{home}`. Paths in these rules that name the home's \
         files, such as `standards/`, are in it.\n"
    );
    text.extend(shared);
    text.extend(own);
    Some(text)
}

fn project(home: &str, own: &Own<'_>) -> String {
    let title = &own.project.title;
    let profiles = if own.project.profiles.is_empty() {
        String::from("It names no profile.\n")
    } else {
        own.project
            .profiles
            .iter()
            .map(|profile| {
                let standards = format!("{home}/standards/{profile}/");
                format!(
                    "- `{profile}`: its checks and configs are in \
                     `{standards}`. Run `{standards}check.sh <tier>` in the \
                     session's directory.\n"
                )
            })
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

fn section(heading: &str, body: &str) -> String {
    format!("\n## {heading}\n\n{}\n", body.trim_end())
}

#[cfg(test)]
mod tests;
