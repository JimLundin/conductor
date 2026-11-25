//! `home check`: the home's projects and issues against its flow.
//!
//! Structural problems are errors: a flow that doesn't load, a project
//! file that doesn't read, a profile two projects provide, an issue the
//! flow can't read. A missing section warns. Each rule reports at its own
//! severity. The check fails only on errors.

use alloc::collections::BTreeMap;
use alloc::string::ToString;

use crate::ProjectId;
use crate::flow::{Flow, Literal, Predicate, Quantifier, Rule, Severity};
use crate::issue::Issue;

use super::eval::{Failure, Move, holds};
use super::snapshot::Snapshot;
use crate::prelude::*;

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
pub fn check(home: &Snapshot) -> Report {
    let error = |subject: Option<String>, message: String| Finding {
        severity: Severity::Error,
        subject,
        message,
    };
    let flow = match home.flow() {
        Ok(flow) => flow,
        Err(problem) => {
            return Report {
                findings: Vec::from([error(None, String::from(problem))]),
            };
        }
    };
    let project_names: BTreeMap<ProjectId, &str> = home
        .projects()
        .iter()
        .map(|project| (project.id, project.name.as_str()))
        .collect();
    let mut findings: Vec<Finding> = home
        .listing_problems()
        .iter()
        .chain(home.unread())
        .map(|problem| error(None, problem.clone()))
        .collect();
    findings.extend(
        providers(home.projects())
            .into_iter()
            .map(|message| error(None, message)),
    );
    let graph = home.graph();
    let named = |project: ProjectId, stem: &str| {
        project_names.get(&project).map_or_else(
            || format!("{project}/{stem}"),
            |name| format!("{name}/{stem}"),
        )
    };
    findings.extend(graph.problems.iter().map(|problem| {
        error(
            Some(named(problem.project, &problem.stem)),
            problem.to_string(),
        )
    }));
    let issues: Vec<&Issue> = graph.issues.iter().collect();
    for issue in &graph.issues {
        let subject = named(issue.project, &issue.id.to_string());
        let standing = Move {
            from: Some(issue),
            to: Some(issue),
        };
        findings.extend(of_issue(flow, &issues, issue, standing, &subject));
    }
    Report { findings }
}

pub(crate) fn of_issue(
    flow: &Flow,
    issues: &[&Issue],
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
            let has = Predicate::HasSection((*section).clone());
            holds(flow, issues, issue, moving, &has).is_err()
        })
        .map(|section| Finding {
            severity: Severity::Warn,
            subject: Some(subject.to_owned()),
            message: format!("lacks the section {section:?}"),
        })
        .collect();
    findings.extend(broken(flow, issues, issue, moving).map(
        |(rule, failure)| Finding {
            severity: rule.severity,
            subject: Some(subject.to_owned()),
            message: format!("{}: {}", rule.message, explain(&failure)),
        },
    ));
    findings
}

pub(crate) fn broken<'a>(
    flow: &'a Flow,
    issues: &'a [&'a Issue],
    issue: &'a Issue,
    moving: Move<'a>,
) -> impl Iterator<Item = (&'a Rule, Failure)> {
    flow.rules.iter().filter_map(move |rule| {
        holds(flow, issues, issue, moving, &rule.when).ok()?;
        holds(flow, issues, issue, moving, &rule.require)
            .err()
            .map(|failure| (rule, failure))
    })
}

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
        format!(
            " (via {})",
            failure
                .path
                .iter()
                .map(|step| format!("{} {}", step.via, step.issue))
                .collect::<Vec<String>>()
                .join(", ")
        )
    };
    format!(
        "{} fails on {}{path}",
        describe(&failure.predicate),
        failure.issue
    )
}

#[must_use]
fn describe(predicate: &Predicate) -> String {
    let list = |word: &str, parts: &[Predicate]| {
        format!(
            "{{ {word} = [{}] }}",
            parts
                .iter()
                .map(describe)
                .collect::<Vec<String>>()
                .join(", ")
        )
    };
    let boxed = |word: &str, inner: &Predicate| {
        format!("{{ {word} = {} }}", describe(inner))
    };
    match predicate {
        Predicate::Kind(kind) => format!("{{ kind = {kind:?} }}"),
        Predicate::Stage(stage) => format!("{{ stage = {stage:?} }}"),
        Predicate::StatusRole(role) => {
            format!("{{ status_role = {:?} }}", role.word())
        }
        Predicate::Field { name, value } => match value {
            Literal::Text(text) => {
                format!("{{ field = {{ {name} = {text:?} }} }}")
            }
            Literal::Bool(flag) => {
                format!("{{ field = {{ {name} = {flag} }} }}")
            }
        },
        Predicate::Has(name) => format!("{{ has = {name:?} }}"),
        Predicate::HasSection(section) => {
            format!("{{ has_section = {section:?} }}")
        }
        Predicate::From(inner) => boxed("from", inner),
        Predicate::To(inner) => boxed("to", inner),
        Predicate::Not(inner) => boxed("not", inner),
        Predicate::All(parts) => list("all", parts),
        Predicate::Any(parts) => list("any", parts),
        Predicate::Across {
            quantifier,
            via,
            condition,
        } => {
            let word = match quantifier {
                Quantifier::Any => "any",
                Quantifier::All => "all",
                Quantifier::None => "none",
            };
            format!(
                "{{ {word} = {{ via = {via:?}, where = {} }} }}",
                describe(condition)
            )
        }
    }
}

#[cfg(test)]
mod tests;
