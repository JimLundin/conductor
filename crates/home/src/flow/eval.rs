//! Whether an issue meets a predicate, among the other issues, and why not
//! when it doesn't.
//!
//! Evaluation follows the predicate, never the issues: each traversal is
//! one step along a relation, so it ends however the issues link, cycles
//! included.

use alloc::string::ToString;

use proto::{Datum, Issue};

use crate::prelude::*;
use proto::flow::{Flow, Literal, Predicate, Quantifier};

/// The issue as it was and as it would be, while a move is weighed.
#[derive(Debug, Clone, Copy, Default)]
pub struct Move<'a> {
    pub from: Option<&'a Issue>,
    pub to: Option<&'a Issue>,
}

/// Why a predicate failed: the atom that didn't hold, on which issue, and
/// the relations followed to reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub atom: String,
    pub issue: String,
    /// Each step as `<relation> <id>`, from the issue evaluated.
    pub path: Vec<String>,
}

/// Whether `issue` meets `predicate` among `issues`, the issues its
/// relations can reach.
///
/// # Errors
///
/// The failure that decides it: for `all`, the first part that fails; for
/// `any`, the first of the parts, all of which fail; across a relation,
/// the first issue reached that decides it, or the relation reaching none.
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
        path: &[String],
    ) -> Result<(), Failure> {
        let fail = |atom: String| Failure {
            atom,
            issue: issue.id.to_string(),
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
                Err(first
                    .unwrap_or_else(|| fail(String::from("any of nothing"))))
            }
            Predicate::Not(inner) => match self.holds(issue, inner, path) {
                Ok(()) => Err(fail(format!("not {}", describe(inner)))),
                Err(_) => Ok(()),
            },
            Predicate::From(inner) => {
                self.moved(self.moving.from, "from", inner, path, fail)
            }
            Predicate::To(inner) => {
                self.moved(self.moving.to, "to", inner, path, fail)
            }
            Predicate::Across {
                quantifier,
                via,
                condition,
            } => self.across(issue, *quantifier, via, condition, path),
            atom => {
                if self.atom(issue, atom) {
                    Ok(())
                } else {
                    Err(fail(describe(atom)))
                }
            }
        }
    }

    fn moved(
        &self,
        side: Option<&Issue>,
        word: &str,
        inner: &Predicate,
        path: &[String],
        fail: impl Fn(String) -> Failure,
    ) -> Result<(), Failure> {
        side.map_or_else(
            || Err(fail(format!("{word}: no move is being weighed"))),
            |issue| self.holds(issue, inner, path),
        )
    }

    fn across(
        &self,
        issue: &Issue,
        quantifier: Quantifier,
        via: &str,
        condition: &Predicate,
        path: &[String],
    ) -> Result<(), Failure> {
        let reached = self.reached(issue, via);
        let step = |target: &Issue| {
            let mut longer = path.to_vec();
            longer.push(format!("{via} {}", target.id));
            longer
        };
        match quantifier {
            Quantifier::All => reached.iter().try_for_each(|target| {
                self.holds(target, condition, &step(target))
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
                Err(first.unwrap_or_else(|| Failure {
                    atom: format!("{via} reaches no issue"),
                    issue: issue.id.to_string(),
                    path: path.to_vec(),
                }))
            }
            Quantifier::None => reached
                .iter()
                .find(|target| {
                    self.holds(target, condition, &step(target)).is_ok()
                })
                .map_or(Ok(()), |target| {
                    Err(Failure {
                        atom: format!("not {}", describe(condition)),
                        issue: target.id.to_string(),
                        path: step(target),
                    })
                }),
        }
    }

    /// The issues one step along `via` from `issue`, forwards along a
    /// relation or backwards along its reverse.
    fn reached(&self, issue: &Issue, via: &str) -> Vec<&Issue> {
        match self.flow.relation(via) {
            None => Vec::new(),
            Some((relation, false)) => issue
                .links
                .get(&relation.name)
                .into_iter()
                .flatten()
                .filter_map(|id| {
                    self.issues.iter().copied().find(|other| other.id == *id)
                })
                .collect(),
            Some((relation, true)) => self
                .issues
                .iter()
                .copied()
                .filter(|other| {
                    other
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

    /// Whether the field is `value`, or holds it for a list; an issue that
    /// leaves the field out has its default.
    fn field(&self, issue: &Issue, name: &str, value: &Literal) -> bool {
        let given = issue.fields.get(name).cloned().or_else(|| {
            self.flow
                .field(&issue.kind, name)
                .and_then(|field| field.default.clone())
                .map(Datum::Text)
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

/// The predicate as the flow writes it.
#[must_use]
pub fn describe(predicate: &Predicate) -> String {
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
