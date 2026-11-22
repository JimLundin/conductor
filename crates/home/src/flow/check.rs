//! `home check`: the home's projects and issues against its flow.
//!
//! Structural problems are errors: a flow that doesn't load, a project
//! file that doesn't read, a profile two projects provide, an issue the
//! flow can't read. A missing section warns. Each rule reports at its own
//! severity. The check fails only on errors.

use alloc::collections::BTreeMap;

use super::eval::{Failure, Move, holds};
use super::graph::{File, Graph, Issue, read};
use super::{Flow, Severity, load};
use crate::prelude::*;
use crate::project::listed;

/// What the check reads, as the shell found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inputs {
    /// `docs/agents/flow.toml`, or why it couldn't be read.
    pub flow: Result<String, String>,
    /// Each folder of `projects/` with its `project.toml`, or why not.
    pub projects: Vec<(String, Result<String, String>)>,
    pub issues: Vec<File>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    /// `<project>/<issue>`, when an issue is the subject.
    pub subject: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    #[must_use]
    pub fn failed(&self) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.severity == Severity::Error)
    }

    /// One line per finding, then the counts.
    #[must_use]
    pub fn text(&self) -> String {
        let mut lines: Vec<String> = self
            .findings
            .iter()
            .map(|finding| {
                let word = match finding.severity {
                    Severity::Error => "error",
                    Severity::Warn => "warn ",
                };
                let subject = finding
                    .subject
                    .as_ref()
                    .map_or_else(String::new, |subject| format!("{subject}  "));
                format!("{word}  {subject}{}\n", finding.message)
            })
            .collect();
        let errors = self
            .findings
            .iter()
            .filter(|finding| finding.severity == Severity::Error)
            .count();
        let warnings = self.findings.len() - errors;
        lines.push(format!("{errors} errors, {warnings} warnings\n"));
        lines.concat()
    }
}

#[must_use]
pub fn check(inputs: &Inputs) -> Report {
    let error = |subject: Option<String>, message: String| Finding {
        severity: Severity::Error,
        subject,
        message,
    };
    let flow = match inputs.flow.as_deref().map_err(Clone::clone).and_then(load)
    {
        Ok(flow) => flow,
        Err(problem) => {
            return Report {
                findings: Vec::from([error(
                    None,
                    format!("docs/agents/flow.toml: {problem}"),
                )]),
            };
        }
    };
    let listing = listed(inputs.projects.clone());
    let project_names: BTreeMap<String, String> = listing
        .projects
        .iter()
        .map(|project| (project.id.0.clone(), project.name.clone()))
        .collect();
    let mut findings: Vec<Finding> = listing
        .problems
        .iter()
        .map(|problem| error(None, problem.clone()))
        .collect();
    findings.extend(
        providers(&listing.projects)
            .into_iter()
            .map(|message| error(None, message)),
    );
    let graph = read(&flow, &inputs.issues);
    let named = |place: &str| {
        place.split_once('/').map_or_else(
            || place.to_owned(),
            |(project, issue)| {
                format!(
                    "{}/{issue}",
                    project_names.get(project).map_or(project, String::as_str)
                )
            },
        )
    };
    findings.extend(graph.problems.iter().map(|problem| {
        let message = problem.field.as_ref().map_or_else(
            || problem.message.clone(),
            |field| format!("{field}: {}", problem.message),
        );
        error(Some(named(&problem.issue)), message)
    }));
    for issue in &graph.issues {
        let subject = named(&format!("{}/{}", issue.project.0, issue.id));
        findings.extend(of_issue(
            &flow,
            &graph,
            issue,
            Move::default(),
            &subject,
        ));
    }
    Report { findings }
}

/// What the flow's sections and rules say of `issue`, weighing `moving`
/// when a move is being made.
pub(crate) fn of_issue(
    flow: &Flow,
    graph: &Graph,
    issue: &Issue,
    moving: Move<'_>,
    subject: &str,
) -> Vec<Finding> {
    let expected = flow
        .kind(&issue.kind)
        .map_or(&[][..], |kind| kind.sections.as_slice());
    let mut findings: Vec<Finding> = expected
        .iter()
        .filter(|section| {
            !issue.sections.iter().any(|had| had.heading == **section)
        })
        .map(|section| Finding {
            severity: Severity::Warn,
            subject: Some(subject.to_owned()),
            message: format!("lacks the section {section:?}"),
        })
        .collect();
    for rule in &flow.rules {
        let applies = holds(flow, graph, issue, moving, &rule.when).is_ok();
        let broken = applies
            .then(|| holds(flow, graph, issue, moving, &rule.require).err())
            .flatten();
        findings.extend(broken.map(|failure| Finding {
            severity: rule.severity,
            subject: Some(subject.to_owned()),
            message: format!("{}: {}", rule.message, explain(&failure)),
        }));
    }
    findings
}

/// A profile that more than one project provides, once each.
fn providers(projects: &[crate::project::Project]) -> Vec<String> {
    let mut by_profile: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for project in projects {
        for profile in &project.provides {
            by_profile.entry(profile).or_default().push(&project.name);
        }
    }
    by_profile
        .into_iter()
        .filter(|(_, names)| names.len() > 1)
        .map(|(profile, names)| {
            format!(
                "profile {profile:?} is provided by more than one project: {}",
                names.join(", ")
            )
        })
        .collect()
}

fn explain(failure: &Failure) -> String {
    let path = if failure.path.is_empty() {
        String::new()
    } else {
        format!(" (via {})", failure.path.join(", "))
    };
    format!("{} fails on {}{path}", failure.atom, failure.issue)
}

#[cfg(test)]
mod tests;
