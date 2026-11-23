//! What the flow's status roles mean to Conductor.
//!
//! Which status a new issue opens in, which issues a session can start
//! on, and where a session moves its issue: Conductor asks these, never a
//! role or a status's name.

use proto::Issue;

use super::check::broken;
use super::eval::Move;
use crate::prelude::*;
use proto::flow::{Flow, Role};

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

/// Whether a session can start on `issue`, among `issues`: it isn't at
/// work already, and moving it to the active status breaks none of the
/// flow's rules.
#[must_use]
pub fn can_start(flow: &Flow, issues: &[&Issue], issue: &Issue) -> bool {
    let Some(active) = holding(flow, Role::Active) else {
        return false;
    };
    if flow.status(&issue.status).map(|status| status.role)
        == Some(Role::Active)
    {
        return false;
    }
    let mut after = issue.clone();
    after.status = String::from(active);
    let moving = Move {
        from: Some(issue),
        to: Some(&after),
    };
    broken(flow, issues, &after, moving).next().is_none()
}

/// The status `issue` moves to once a session on it has started, among
/// `issues`: one that can start becomes active.
#[must_use]
pub fn started<'a>(
    flow: &'a Flow,
    issues: &[&Issue],
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
