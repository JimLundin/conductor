use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use home::flow::Flow;
use home::issue::{RelationChange, RelationStep};
use home::tracker::Ids;
use home::{IssueKey, ProjectId};

use super::relations::{BLOCKED_BY, PARENT};
use super::{Native, Record, footer};
use crate::IssueNumber;
use crate::access::Repositories;

pub const SUB_ISSUES: u32 = 100;
pub const LEVELS: u32 = 8;
pub const DEPENDENCIES: u32 = 50;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Unheld {
    #[error("no relation is called {0:?}")]
    Unknown(String),
    #[error("relation {0:?} points to one issue, not a list")]
    Single(String),
    #[error("GitHub reported no dependencies for this issue")]
    Unread,
    #[error("issue {0} already has {SUB_ISSUES} sub-issues")]
    SubIssues(Native),
    #[error("sub-issues nest at most {LEVELS} levels deep")]
    Levels,
    #[error("an issue can be blocked by at most {DEPENDENCIES} issues")]
    BlockedBy,
    #[error("issue {0} already blocks {DEPENDENCIES} issues")]
    Blocking(u64),
    #[error("the project-data footer: {0}")]
    Footer(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Adopt { parent: Native, replace: bool },
    Orphan(Native),
    Block(u64),
    Unblock(u64),
}

impl Change {
    #[must_use]
    pub fn effect(&self) -> String {
        match self {
            Self::Adopt { parent, .. } => format!("sub-issue of {parent}"),
            Self::Orphan(parent) => format!("not a sub-issue of {parent}"),
            Self::Block(blocker) => format!("blocked by {blocker}"),
            Self::Unblock(blocker) => format!("not blocked by {blocker}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub changes: Vec<Change>,
    pub footer: Option<RelationStep>,
}

pub fn plan(
    flow: &Flow,
    ids: &Ids,
    repositories: &Repositories,
    project: ProjectId,
    record: &Record,
    change: &RelationChange,
) -> Result<Plan, Unheld> {
    let name = &change.relation;
    let Some((relation, false)) = flow.relation(name) else {
        return Err(Unheld::Unknown(name.clone()));
    };
    if !relation.many && change.targets.len() > 1 {
        return Err(Unheld::Single(name.clone()));
    }
    let located = |target: &IssueKey| {
        let number = held(target)?;
        if target.project == project {
            return Some(Native::Here(number));
        }
        let here = repositories.repository(project)?;
        let there = repositories.repository(target.project)?;
        (name == PARENT && here.owner().eq_ignore_ascii_case(there.owner()))
            .then(|| Native::Elsewhere(there.clone(), number))
    };
    let native_kind = matches!(name.as_str(), PARENT | BLOCKED_BY);
    let (native, kept): (Vec<_>, Vec<_>) = change
        .targets
        .iter()
        .cloned()
        .partition(|target| native_kind && located(target).is_some());
    let wanted: Vec<(IssueKey, Native)> = native
        .into_iter()
        .filter_map(|target| located(&target).map(|at| (target, at)))
        .collect();
    let changes = match name.as_str() {
        PARENT => parent(
            record
                .parent
                .as_ref()
                .map(|had| (had, had.key(project, repositories))),
            wanted.into_iter().next(),
        ),
        BLOCKED_BY => blockers(
            record.blocked_by.as_deref(),
            &wanted
                .iter()
                .filter_map(|(_, at)| match at {
                    Native::Here(number) => Some(*number),
                    Native::Elsewhere(..) => None,
                })
                .collect::<Vec<_>>(),
        )?,
        _ => Vec::new(),
    };
    let footer =
        footer::relate(&record.native.body, relation, project, ids, &kept)
            .map_err(Unheld::Footer)?
            .map(|body| RelationStep {
                effect: format!(
                    "footer {name} = [{}]",
                    kept.iter()
                        .map(|target| target.said_from(project))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                request: Request::Body(body).encode(),
            });
    Ok(Plan { changes, footer })
}

fn held(target: &IssueKey) -> Option<u64> {
    target.id.tracked::<IssueNumber>().map(IssueNumber::get)
}

fn parent(
    current: Option<(&Native, Option<IssueKey>)>,
    wanted: Option<(IssueKey, Native)>,
) -> Vec<Change> {
    match (current, wanted) {
        (Some((_, Some(had))), Some((key, _))) if had == key => Vec::new(),
        (Some(_), Some((_, parent))) => Vec::from([Change::Adopt {
            parent,
            replace: true,
        }]),
        (None, Some((_, parent))) => Vec::from([Change::Adopt {
            parent,
            replace: false,
        }]),
        (Some((had, Some(_))), None) => {
            Vec::from([Change::Orphan(had.clone())])
        }
        (Some((_, None)) | None, None) => Vec::new(),
    }
}

fn blockers(
    read: Option<&[Native]>,
    wanted: &[u64],
) -> Result<Vec<Change>, Unheld> {
    let current = read.ok_or(Unheld::Unread)?;
    let here: Vec<u64> = current
        .iter()
        .filter_map(|edge| match edge {
            Native::Here(number) => Some(*number),
            Native::Elsewhere(..) => None,
        })
        .collect();
    let elsewhere = current.len() - here.len();
    if u32::try_from(wanted.len() + elsewhere)
        .map_or(true, |count| count > DEPENDENCIES)
    {
        return Err(Unheld::BlockedBy);
    }
    Ok(wanted
        .iter()
        .filter(|blocker| !here.contains(blocker))
        .map(|blocker| Change::Block(*blocker))
        .chain(
            here.iter()
                .filter(|blocker| !wanted.contains(blocker))
                .map(|blocker| Change::Unblock(*blocker)),
        )
        .collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parent {
    pub at: Native,
    pub sub_issues: u32,
    pub ancestors: u32,
}

pub fn adoptable(parent: Parent, height: u32) -> Result<(), Unheld> {
    if parent.sub_issues >= SUB_ISSUES {
        return Err(Unheld::SubIssues(parent.at));
    }
    if parent.ancestors + 1 + height > LEVELS {
        return Err(Unheld::Levels);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nested {
    pub at: Native,
    pub sub_issues: Option<u32>,
}

#[must_use]
pub fn deeper(height: u32, level: Vec<Nested>) -> Option<Vec<Native>> {
    if level.is_empty() || height > LEVELS {
        return None;
    }
    Some(
        level
            .into_iter()
            .filter(|child| child.sub_issues != Some(0))
            .map(|child| child.at)
            .collect(),
    )
}

pub const fn blockable(blocker: u64, blocking: u32) -> Result<(), Unheld> {
    if blocking >= DEPENDENCIES {
        return Err(Unheld::Blocking(blocker));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Adopt {
        parent: Native,
        child: u64,
        replace: bool,
    },
    Orphan {
        parent: Native,
        child: u64,
    },
    Block(u64),
    Unblock(u64),
    Body(String),
}

impl Request {
    #[must_use]
    pub fn encode(&self) -> String {
        match self {
            Self::Adopt {
                parent,
                child,
                replace,
            } => format!("adopt {parent} {child} {replace}"),
            Self::Orphan { parent, child } => {
                format!("orphan {parent} {child}")
            }
            Self::Block(blocker) => format!("block {blocker}"),
            Self::Unblock(blocker) => format!("unblock {blocker}"),
            Self::Body(body) => format!("body\n{body}"),
        }
    }

    #[must_use]
    pub fn decode(text: &str) -> Option<Self> {
        let (head, body) = text.split_once('\n').unwrap_or((text, ""));
        let words: Vec<&str> = head.split(' ').collect();
        let at =
            |index: usize| -> Option<u64> { words.get(index)?.parse().ok() };
        Some(match words.first().copied()? {
            "adopt" => Self::Adopt {
                parent: Native::read(words.get(1)?)?,
                child: at(2)?,
                replace: words.get(3)?.parse().ok()?,
            },
            "orphan" => Self::Orphan {
                parent: Native::read(words.get(1)?)?,
                child: at(2)?,
            },
            "block" => Self::Block(at(1)?),
            "unblock" => Self::Unblock(at(1)?),
            "body" => Self::Body(body.to_string()),
            _ => return None,
        })
    }
}
