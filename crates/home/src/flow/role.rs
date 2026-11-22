//! What the flow's status roles mean to Conductor.
//!
//! Which status a new issue opens in, which issues a session can start
//! on, and where a session moves its issue: Conductor asks these, never a
//! role or a status's name.

use alloc::boxed::Box;

use proto::Issue;

use super::eval::{self, Among};
use super::{Flow, Predicate, Quantifier, Role, Shape};

/// The status that holds `role`: the first with it the flow declares.
fn holding(flow: &Flow, role: Role) -> Option<&str> {
    flow.statuses
        .iter()
        .find(|status| status.role == role)
        .map(|status| status.name.as_str())
}

/// The status a new issue opens in.
#[must_use]
pub fn opening(flow: &Flow) -> Option<&str> {
    holding(flow, Role::Idle)
}

/// Whether a session can start on `issue`, among `issues`: it waits to be
/// taken up, and every issue it depends on, through each of the flow's
/// dependency relations, is built or dropped.
#[must_use]
pub fn can_start<A: Among + ?Sized>(
    flow: &Flow,
    issues: &A,
    issue: &Issue,
) -> bool {
    eval::holds(flow, issues, issue, eval::Move::default(), &startable(flow))
        .is_ok()
}

/// What [`can_start`] asks of an issue.
fn startable(flow: &Flow) -> Predicate {
    let built = any_of(&[Role::Review, Role::Terminal, Role::Dropped]);
    let blockers = flow
        .relations
        .iter()
        .filter(|relation| relation.shape == Shape::Dependency)
        .map(|relation| Predicate::Across {
            quantifier: Quantifier::All,
            via: relation.name.clone(),
            condition: Box::new(built.clone()),
        });
    Predicate::All(
        core::iter::once(any_of(&[Role::Idle, Role::Startable]))
            .chain(blockers)
            .collect(),
    )
}

fn any_of(roles: &[Role]) -> Predicate {
    Predicate::Any(roles.iter().copied().map(Predicate::StatusRole).collect())
}

/// The status `issue` moves to once a session on it has started, among
/// `issues`: one that can start becomes active.
#[must_use]
pub fn started<'a, A: Among + ?Sized>(
    flow: &'a Flow,
    issues: &A,
    issue: &Issue,
) -> Option<&'a str> {
    can_start(flow, issues, issue)
        .then(|| holding(flow, Role::Active))
        .flatten()
}

/// The status `issue` moves to once a session on it has finished with
/// work landed on its branch: one not yet built is up for review.
#[must_use]
pub fn landed<'a>(flow: &'a Flow, issue: &Issue) -> Option<&'a str> {
    let role = flow.status(&issue.status)?.role;
    matches!(role, Role::Idle | Role::Startable | Role::Active)
        .then(|| holding(flow, Role::Review))
        .flatten()
}

#[cfg(test)]
mod tests;
