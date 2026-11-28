use super::{Flow, Predicate};
use crate::issue::{Coverage, Observation};
use alloc::collections::BTreeSet;

pub(super) fn applicable(flow: &Flow, records: &[Observation]) -> Flow {
    let mut fields = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let valid: BTreeSet<_> = records
        .iter()
        .filter_map(Observation::issue)
        .map(|issue| (issue.project, &issue.id))
        .collect();
    for record in records {
        for (name, coverage) in &record.completeness.relations {
            if matches!(coverage, Coverage::Incomplete { .. }) {
                fields.insert(name.as_str());
                paths.insert(name.as_str());
            }
        }
        if let Some(issue) = record.issue() {
            for (name, targets) in &issue.links {
                if targets
                    .iter()
                    .any(|id| !valid.contains(&(issue.project, id)))
                {
                    paths.insert(name.as_str());
                }
            }
        }
    }
    paths.extend(
        flow.relations
            .iter()
            .map(|relation| relation.reverse.as_str()),
    );
    let mut applicable = flow.clone();
    applicable.rules.retain(|rule| {
        observed(&rule.when, &fields, &paths)
            && observed(&rule.require, &fields, &paths)
    });
    applicable
}

fn observed(
    predicate: &Predicate,
    fields: &BTreeSet<&str>,
    paths: &BTreeSet<&str>,
) -> bool {
    match predicate {
        Predicate::Has(name) => !fields.contains(name.as_str()),
        Predicate::Across { via, condition, .. } => {
            !paths.contains(via.as_str()) && observed(condition, fields, paths)
        }
        Predicate::From(inner)
        | Predicate::To(inner)
        | Predicate::Not(inner) => observed(inner, fields, paths),
        Predicate::All(parts) | Predicate::Any(parts) => {
            parts.iter().all(|part| observed(part, fields, paths))
        }
        Predicate::Kind(_)
        | Predicate::Stage(_)
        | Predicate::StatusRole(_)
        | Predicate::Field { .. }
        | Predicate::HasSection(_) => true,
    }
}
