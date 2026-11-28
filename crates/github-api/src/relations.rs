use github::access::routes::Address;
use github::mapping::relate::{
    self, Change, LEVELS, Nested, Parent, Request, Unheld, adoptable, blockable,
};
use home::issue::{
    Lookup, ReadFailure, Relating, RelationChange, RelationStep, SendFailure,
    Unrelatable,
};

use crate::Tracker;
use crate::client::unsent;
use crate::tracking::{ReturnedIssue, request_error};

#[derive(serde::Serialize)]
struct SubIssue {
    sub_issue_id: u64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    replace_parent: bool,
}

#[derive(serde::Serialize)]
struct Blocker {
    issue_id: u64,
}

#[derive(serde::Serialize)]
struct Body<'a> {
    body: &'a str,
}

fn unheld(reason: &Unheld) -> Unrelatable {
    Unrelatable::Unheld(reason.to_string())
}

impl Tracker {
    pub(crate) async fn relating(
        &self,
        issue: Lookup,
        change: RelationChange,
    ) -> Result<Relating, Unrelatable> {
        let (record, observed) =
            self.current(&issue).await.map_err(Unrelatable::Read)?;
        let revision = observed
            .revision
            .ok_or(Unrelatable::Read(ReadFailure::InvalidResponse))?;
        let plan = relate::plan(&self.flow, issue.project, &record, &change)
            .map_err(|reason| unheld(&reason))?;
        let child = record
            .locator
            .addresses
            .get("id")
            .and_then(|id| id.parse::<u64>().ok())
            .ok_or(Unrelatable::Read(ReadFailure::InvalidResponse))?;
        let number =
            Address::of(&issue.id).map_err(Unrelatable::Read)?.number();
        let mut steps = Vec::new();
        for edge in plan.changes {
            let request = match edge {
                Change::Adopt { parent, replace } => {
                    let facts = self.parent(parent).await?;
                    let height = self.height(number).await?;
                    adoptable(facts, height)
                        .map_err(|reason| unheld(&reason))?;
                    Request::Adopt {
                        parent,
                        child,
                        replace,
                    }
                }
                Change::Orphan(parent) => Request::Orphan { parent, child },
                Change::Block(blocker) => {
                    let target = self.issue(blocker).await?;
                    let blocking = target
                        .issue_dependencies_summary
                        .map(|summary| summary.total_blocking)
                        .ok_or(Unrelatable::Read(
                            ReadFailure::InvalidResponse,
                        ))?;
                    blockable(blocker, blocking)
                        .map_err(|reason| unheld(&reason))?;
                    Request::Block(target.issue.id.0)
                }
                Change::Unblock(blocker) => {
                    Request::Unblock(self.issue(blocker).await?.issue.id.0)
                }
            };
            steps.push(RelationStep {
                effect: edge.effect(),
                request: request.encode(),
            });
        }
        steps.extend(plan.footer);
        Ok(Relating { revision, steps })
    }

    async fn issue(&self, number: u64) -> Result<ReturnedIssue, Unrelatable> {
        let route =
            format!("{}/{number}", self.client.repository.issues_route());
        self.returned(&route).await
    }

    async fn returned(
        &self,
        route: &str,
    ) -> Result<ReturnedIssue, Unrelatable> {
        self.client
            .reader
            .get(route, None::<&()>)
            .await
            .map_err(|error| Unrelatable::Read(request_error(error)))
    }

    async fn parent(&self, number: u64) -> Result<Parent, Unrelatable> {
        let found = self.issue(number).await?;
        let sub_issues = found
            .sub_issues_summary
            .as_ref()
            .map(|summary| summary.total)
            .ok_or(Unrelatable::Read(ReadFailure::InvalidResponse))?;
        let mut ancestors = 0;
        let mut above = found.parent_issue_url;
        while let Some(url) = above {
            ancestors += 1;
            if ancestors > LEVELS {
                break;
            }
            let route = self
                .client
                .repository
                .issue_route(&self.client.endpoint, &url)
                .map_err(Unrelatable::Read)?;
            above = self.returned(&route).await?.parent_issue_url;
        }
        Ok(Parent {
            number,
            sub_issues,
            ancestors,
        })
    }

    async fn height(&self, number: u64) -> Result<u32, Unrelatable> {
        let mut level = Vec::from([number]);
        let mut height = 0;
        while let Some(below) =
            relate::deeper(height, self.level(&level).await?)
        {
            height += 1;
            level = below;
        }
        Ok(height)
    }

    async fn level(&self, parents: &[u64]) -> Result<Vec<Nested>, Unrelatable> {
        let mut found = Vec::new();
        for parent in parents {
            found.extend(self.children(*parent).await?);
        }
        Ok(found)
    }

    async fn children(&self, number: u64) -> Result<Vec<Nested>, Unrelatable> {
        let page: octocrab::Page<ReturnedIssue> = self
            .client
            .reader
            .get(
                self.client.repository.sub_issues_route(number),
                Some(&[("per_page", 100)]),
            )
            .await
            .map_err(|error| Unrelatable::Read(request_error(error)))?;
        if page.next.is_some() {
            return Err(Unrelatable::Read(ReadFailure::InvalidResponse));
        }
        Ok(page
            .items
            .into_iter()
            .map(|child| Nested {
                number: child.issue.number,
                sub_issues: child
                    .sub_issues_summary
                    .map(|summary| summary.total),
            })
            .collect())
    }

    pub(crate) async fn relate(
        &self,
        issue: Lookup,
        step: RelationStep,
    ) -> Result<(), SendFailure> {
        let number = Address::of(&issue.id)
            .map_err(SendFailure::Rejected)?
            .number();
        let request = Request::decode(&step.request)
            .ok_or(SendFailure::Rejected(ReadFailure::InvalidConfiguration))?;
        let repository = &self.client.repository;
        let reader = &self.client.reader;
        let sent = match request {
            Request::Adopt {
                parent,
                child,
                replace,
            } => {
                reader
                    ._post(
                        repository.sub_issues_route(parent),
                        Some(&SubIssue {
                            sub_issue_id: child,
                            replace_parent: replace,
                        }),
                    )
                    .await
            }
            Request::Orphan { parent, child } => {
                reader
                    ._delete(
                        repository.sub_issue_route(parent),
                        Some(&SubIssue {
                            sub_issue_id: child,
                            replace_parent: false,
                        }),
                    )
                    .await
            }
            Request::Block(blocker) => {
                reader
                    ._post(
                        repository.blockers_route(number),
                        Some(&Blocker { issue_id: blocker }),
                    )
                    .await
            }
            Request::Unblock(blocker) => {
                reader
                    ._delete(
                        format!(
                            "{}/{blocker}",
                            repository.blockers_route(number)
                        ),
                        None::<&()>,
                    )
                    .await
            }
            Request::Body(body) => {
                reader
                    ._patch(
                        format!("{}/{number}", repository.issues_route()),
                        Some(&Body { body: &body }),
                    )
                    .await
            }
        };
        octocrab::map_github_error(sent.map_err(unsent)?)
            .await
            .map_err(unsent)?;
        Ok(())
    }
}
