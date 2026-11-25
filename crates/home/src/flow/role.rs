use crate::issue::Issue;

use super::check::broken;
use super::eval::Move;
use crate::flow::{Flow, Role};
use crate::prelude::*;

fn holding(flow: &Flow, role: Role) -> Option<&str> {
    flow.statuses
        .iter()
        .find(|status| status.role == role)
        .map(|status| status.name.as_str())
}

#[must_use]
pub fn opening(flow: &Flow) -> Option<&str> {
    holding(flow, Role::Idle)
}

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

#[must_use]
pub fn landed<'a>(flow: &'a Flow, issue: &Issue) -> Option<&'a str> {
    let role = flow.status(&issue.status)?.role;
    matches!(role, Role::Idle | Role::Startable | Role::Active)
        .then(|| holding(flow, Role::Review))
        .flatten()
}

#[cfg(test)]
mod tests;
