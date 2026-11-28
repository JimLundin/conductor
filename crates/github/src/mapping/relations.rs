use alloc::{collections::BTreeMap, format, string::String, vec::Vec};
use home::flow::Flow;
use home::issue::Coverage;
use home::{IssueKey, ProjectId};

use super::{Findings, Record};
use crate::access::Repositories;

pub(super) const PARENT: &str = "parent";
pub(super) const BLOCKED_BY: &str = "blocked_by";

pub(super) struct Relations {
    pub links: BTreeMap<String, Vec<IssueKey>>,
    pub coverage: BTreeMap<String, Coverage>,
}

impl Relations {
    pub(super) fn read(
        flow: &Flow,
        repositories: &Repositories,
        project: ProjectId,
        record: &Record,
        mut footer: BTreeMap<String, Vec<IssueKey>>,
        findings: &mut Findings,
    ) -> Self {
        let mut relations = Self {
            links: BTreeMap::new(),
            coverage: BTreeMap::new(),
        };
        for relation in &flow.relations {
            let name = &relation.name;
            let native = match name.as_str() {
                PARENT => Some(record.parent.as_slice()),
                BLOCKED_BY => record.blocked_by.as_deref(),
                _ => Some(&[][..]),
            };
            let mut coverage = native.map_or_else(
                || Coverage::Incomplete {
                    reason: String::from(
                        "GitHub reported no dependencies for this issue",
                    ),
                },
                |_| Coverage::Complete,
            );
            let mut targets = Vec::new();
            for edge in native.unwrap_or_default() {
                if let Some(target) = edge.key(project, repositories) {
                    targets.push(target);
                } else {
                    findings.warn(format!(
                        "relation {name:?} names {edge}, in a repository no project tracks"
                    ));
                    coverage = Coverage::Incomplete {
                        reason: format!(
                            "{edge} is in a repository no project tracks"
                        ),
                    };
                }
            }
            for target in footer.remove(name).unwrap_or_default() {
                if !targets.contains(&target) {
                    targets.push(target);
                }
            }
            if !relation.many && targets.len() > 1 {
                findings.error(format!(
                    "relation {name:?} names more than one issue: {}",
                    targets
                        .iter()
                        .map(|target| target.said_from(project))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if !targets.is_empty() {
                relations.links.insert(name.clone(), targets);
            }
            relations.coverage.insert(name.clone(), coverage);
        }
        relations
    }
}
