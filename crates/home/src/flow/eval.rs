use alloc::boxed::Box;

use crate::IssueId;
use crate::issue::{Datum, Issue};

use crate::flow::{Flow, Literal, Predicate, Quantifier};
use crate::prelude::*;

#[derive(Debug, Clone, Copy, Default)]
pub struct Move<'a> {
    pub from: Option<&'a Issue>,
    pub to: Option<&'a Issue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub predicate: Predicate,
    pub issue: IssueId,
    pub path: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub via: String,
    pub issue: IssueId,
}

pub fn holds(
    flow: &Flow,
    issues: &[&Issue],
    issue: &Issue,
    moving: Move<'_>,
    predicate: &Predicate,
) -> Result<(), Failure> {
    Walk {
        flow,
        issues,
        moving,
    }
    .holds(issue, predicate, &[])
}

struct Walk<'a> {
    flow: &'a Flow,
    issues: &'a [&'a Issue],
    moving: Move<'a>,
}

impl Walk<'_> {
    fn holds(
        &self,
        issue: &Issue,
        predicate: &Predicate,
        path: &[Step],
    ) -> Result<(), Failure> {
        let fail = |failed: &Predicate| Failure {
            predicate: failed.clone(),
            issue: issue.id.clone(),
            path: path.to_vec(),
        };
        match predicate {
            Predicate::All(parts) => parts
                .iter()
                .try_for_each(|part| self.holds(issue, part, path)),
            Predicate::Any(parts) => {
                let mut first = None;
                for part in parts {
                    match self.holds(issue, part, path) {
                        Ok(()) => return Ok(()),
                        Err(failure) => {
                            first.get_or_insert(failure);
                        }
                    }
                }
                Err(first.unwrap_or_else(|| fail(predicate)))
            }
            Predicate::Not(inner) => match self.holds(issue, inner, path) {
                Ok(()) => Err(fail(predicate)),
                Err(_) => Ok(()),
            },
            Predicate::From(inner) => self.moving.from.map_or_else(
                || Err(fail(predicate)),
                |from| self.holds(from, inner, path),
            ),
            Predicate::To(inner) => self.moving.to.map_or_else(
                || Err(fail(predicate)),
                |to| self.holds(to, inner, path),
            ),
            Predicate::Across {
                quantifier,
                via,
                condition,
            } => self
                .across(issue, *quantifier, via, condition, path)
                .map_err(|reached| reached.unwrap_or_else(|| fail(predicate))),
            atom => {
                if self.atom(issue, atom) {
                    Ok(())
                } else {
                    Err(fail(atom))
                }
            }
        }
    }

    fn across(
        &self,
        issue: &Issue,
        quantifier: Quantifier,
        via: &str,
        condition: &Predicate,
        path: &[Step],
    ) -> Result<(), Option<Failure>> {
        let reached = self.reached(issue, via);
        let step = |target: &Issue| {
            let mut longer = path.to_vec();
            longer.push(Step {
                via: String::from(via),
                issue: target.id.clone(),
            });
            longer
        };
        match quantifier {
            Quantifier::All => reached.iter().try_for_each(|target| {
                self.holds(target, condition, &step(target)).map_err(Some)
            }),
            Quantifier::Any => {
                let mut first = None;
                for target in &reached {
                    match self.holds(target, condition, &step(target)) {
                        Ok(()) => return Ok(()),
                        Err(failure) => {
                            first.get_or_insert(failure);
                        }
                    }
                }
                Err(first)
            }
            Quantifier::None => reached
                .iter()
                .find(|target| {
                    self.holds(target, condition, &step(target)).is_ok()
                })
                .map_or(Ok(()), |target| {
                    Err(Some(Failure {
                        predicate: Predicate::Not(Box::new(condition.clone())),
                        issue: target.id.clone(),
                        path: step(target),
                    }))
                }),
        }
    }

    fn reached(&self, issue: &Issue, via: &str) -> Vec<&Issue> {
        match self.flow.relation(via) {
            None => Vec::new(),
            Some((relation, false)) => issue
                .links
                .get(&relation.name)
                .into_iter()
                .flatten()
                .filter_map(|id| {
                    self.issues.iter().copied().find(|other| {
                        other.project == issue.project && other.id == *id
                    })
                })
                .collect(),
            Some((relation, true)) => self
                .issues
                .iter()
                .copied()
                .filter(|other| {
                    other.project == issue.project
                        && other
                            .links
                            .get(&relation.name)
                            .is_some_and(|ids| ids.contains(&issue.id))
                })
                .collect(),
        }
    }

    fn atom(&self, issue: &Issue, atom: &Predicate) -> bool {
        match atom {
            Predicate::Kind(kind) => issue.kind == *kind,
            Predicate::Stage(stage) => issue.stage.as_ref() == Some(stage),
            Predicate::StatusRole(role) => self
                .flow
                .status(&issue.status)
                .is_some_and(|status| status.role == *role),
            Predicate::Field { name, value } => self.field(issue, name, value),
            Predicate::Has(name) => {
                (name == "stage" && issue.stage.is_some())
                    || issue.fields.contains_key(name)
                    || issue.links.get(name).is_some_and(|ids| !ids.is_empty())
            }
            Predicate::HasSection(section) => {
                issue.sections.iter().any(|had| had.heading == *section)
            }
            _ => false,
        }
    }

    fn field(&self, issue: &Issue, name: &str, value: &Literal) -> bool {
        let given = issue.fields.get(name).cloned().or_else(|| {
            self.flow
                .field(&issue.kind, name)
                .and_then(|field| field.default.clone())
        });
        match (given, value) {
            (Some(Datum::Text(text)), Literal::Text(wanted)) => text == *wanted,
            (Some(Datum::Texts(texts)), Literal::Text(wanted)) => {
                texts.contains(wanted)
            }
            (Some(Datum::Bool(flag)), Literal::Bool(wanted)) => flag == *wanted,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests;
