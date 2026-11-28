use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use github::access::routes::{Address, Resolution};
use github::configuration::Configuration;
use github::mapping::{self, Native, Record};
use home::flow::Flow;
use home::issue::{Comment, CommentWrite, RelationChange, RelationStep};
use home::issue::{
    IssueState, ListPage, Locator, Lookup, Observation, Page, ReadFailure,
    Unreadable,
};
use home::{IssueId, ProjectId, UnixMillis};
use octocrab::{FromResponse, models::issues::Issue, params};
use project_runtime::{
    IssueCompletion, IssueTracker, RelationCompletion, SendCompletion,
};

use crate::{Client, client::classify, read_failure};

pub struct Tracker {
    pub(crate) client: Client,
    pub(crate) flow: Flow,
    config: Configuration,
}

impl Client {
    #[must_use]
    pub const fn tracker(self, flow: Flow, config: Configuration) -> Tracker {
        Tracker {
            client: self,
            flow,
            config,
        }
    }
}

#[derive(serde::Deserialize)]
pub struct ReturnedIssue {
    state_reason: Option<String>,
    pub parent_issue_url: Option<String>,
    pub issue_dependencies_summary: Option<Dependencies>,
    pub sub_issues_summary: Option<SubIssues>,
    #[serde(flatten)]
    pub issue: Issue,
}

#[derive(serde::Deserialize)]
pub struct Dependencies {
    total_blocked_by: u32,
    pub total_blocking: u32,
}

#[derive(serde::Deserialize)]
pub struct SubIssues {
    pub total: u32,
}

#[derive(serde::Deserialize)]
struct Blocker {
    url: String,
}

enum Response {
    Issue(Box<ReturnedIssue>),
    Redirect(String),
    Gone,
}

enum Located {
    Found(Box<Record>),
    Gone(Box<Observation>),
}

impl Tracker {
    async fn page(&self, request: ListPage) -> Result<Page, ReadFailure> {
        let client = &self.client;
        let number = request
            .continuation
            .as_deref()
            .unwrap_or("1")
            .parse::<std::num::NonZeroU32>()
            .map_err(|_| ReadFailure::InvalidResponse)?
            .get();
        let issues = client
            .reader
            .issues(client.repository.owner(), client.repository.name());
        let parameters = issues
            .list()
            .state(params::State::All)
            .sort(params::issues::Sort::Created)
            .direction(params::Direction::Ascending)
            .per_page(100u8)
            .page(number);
        let raw: octocrab::Page<ReturnedIssue> = client
            .reader
            .get(client.repository.issues_route(), Some(&parameters))
            .await
            .map_err(request_error)?;
        let link = raw.next.map(|next| next.to_string());
        let next = client.repository.continuation(
            &client.endpoint,
            number,
            link.as_deref(),
            raw.incomplete_results,
        )?;
        let issues: Vec<_> = raw
            .items
            .into_iter()
            .filter(|returned| returned.issue.pull_request.is_none())
            .collect();
        let mut blocked = Vec::new();
        for returned in &issues {
            blocked.push(self.blocked_by(returned).await?);
        }
        let records = issues
            .into_iter()
            .zip(blocked)
            .map(|(returned, blocked_by)| {
                self.record(returned, blocked_by)
                    .map(|record| self.mapped(request.project, record))
            })
            .collect::<Result<_, _>>()?;
        Ok(Page { records, next })
    }

    async fn blocked_by(
        &self,
        value: &ReturnedIssue,
    ) -> Result<Option<Vec<Native>>, ReadFailure> {
        match value.issue_dependencies_summary {
            None => Ok(None),
            Some(Dependencies {
                total_blocked_by: 0,
                ..
            }) => Ok(Some(Vec::new())),
            Some(_) => self.blockers(value.issue.number).await.map(Some),
        }
    }

    fn record(
        &self,
        value: ReturnedIssue,
        blocked_by: Option<Vec<Native>>,
    ) -> Result<Record, ReadFailure> {
        let client = &self.client;
        let issue = value.issue;
        client.repository.verify_record(
            &client.endpoint,
            issue.repository_url.as_str(),
            issue.url.as_str(),
            issue.number,
        )?;
        let parent = value
            .parent_issue_url
            .as_deref()
            .map(|url| client.repository.native(&client.endpoint, url))
            .transpose()?;
        let id = IssueId::new(&issue.number.to_string())
            .ok_or(ReadFailure::InvalidResponse)?;
        let owned_repo = format!(
            "{}/{}",
            client.repository.owner(),
            client.repository.name()
        );
        let locator = Locator {
            url: Some(format!(
                "https://github.com/{owned_repo}/issues/{}",
                issue.number
            )),
            repository: owned_repo,
            addresses: BTreeMap::from([
                (String::from("id"), issue.id.to_string()),
                (String::from("node_id"), issue.node_id),
            ]),
        };
        Ok(Record {
            id,
            locator,
            title: issue.title,
            body: issue.body.unwrap_or_default(),
            created: u64::try_from(issue.created_at.timestamp_millis())
                .map(UnixMillis)
                .map_err(|_| ReadFailure::InvalidResponse)?,
            open: matches!(issue.state, octocrab::models::IssueState::Open),
            state_reason: value.state_reason,
            labels: issue.labels.into_iter().map(|label| label.name).collect(),
            parent,
            blocked_by,
        })
    }

    fn mapped(&self, project: ProjectId, record: Record) -> Observation {
        let mut observation =
            mapping::observe(&self.flow, &self.config, project, record);
        observation.observed_at = observed_at();
        observation
    }

    async fn blockers(&self, number: u64) -> Result<Vec<Native>, ReadFailure> {
        let client = &self.client;
        let page: octocrab::Page<Blocker> = client
            .reader
            .get(
                client.repository.blockers_route(number),
                Some(&[("per_page", 100)]),
            )
            .await
            .map_err(request_error)?;
        if page.next.is_some() {
            return Err(ReadFailure::InvalidResponse);
        }
        page.items
            .iter()
            .map(|blocker| {
                client.repository.native(&client.endpoint, &blocker.url)
            })
            .collect()
    }

    async fn fetch(&self, address: Address) -> Result<Response, ReadFailure> {
        let response = self
            .client
            .reader
            ._get(address.route(&self.client.repository))
            .await
            .map_err(request_error)?;
        match response.status().as_u16() {
            301 | 302 | 307 | 308 => response
                .headers()
                .get("location")
                .and_then(|header| header.to_str().ok())
                .map(|location| Response::Redirect(location.to_owned()))
                .ok_or(ReadFailure::InvalidResponse),
            410 => Ok(Response::Gone),
            _ => ReturnedIssue::from_response(
                octocrab::map_github_error(response)
                    .await
                    .map_err(request_error)?,
            )
            .await
            .map(Box::new)
            .map(Response::Issue)
            .map_err(request_error),
        }
    }

    async fn redirect(&self, location: &str) -> Result<Address, ReadFailure> {
        let address = self
            .client
            .repository
            .redirect(&self.client.endpoint, location)?;
        if let Address::RepositoryId { repository, .. } = address
            && self.client.metadata().await.map_err(read_failure)?.id.0
                != repository
        {
            return Err(ReadFailure::IdentityConflict);
        }
        Ok(address)
    }

    async fn lookup(
        &self,
        request: Lookup,
    ) -> Result<Observation, ReadFailure> {
        match self.located(&request).await? {
            Located::Gone(observation) => Ok(*observation),
            Located::Found(record) => self.verified(&request, *record),
        }
    }

    pub(crate) async fn current(
        &self,
        request: &Lookup,
    ) -> Result<(Record, Observation), ReadFailure> {
        match self.located(request).await? {
            Located::Gone(_) => Err(ReadFailure::Deleted),
            Located::Found(record) => {
                let observed = self.verified(request, (*record).clone())?;
                Ok((*record, observed))
            }
        }
    }

    fn verified(
        &self,
        request: &Lookup,
        record: Record,
    ) -> Result<Observation, ReadFailure> {
        let observed = self.mapped(request.project, record);
        home::issue::cache::verify(request, &observed)?;
        Ok(observed)
    }

    async fn located(&self, request: &Lookup) -> Result<Located, ReadFailure> {
        let mut resolution = Resolution::start(Address::of(&request.id)?);
        loop {
            match self.fetch(resolution.address).await? {
                Response::Redirect(location) => {
                    resolution =
                        resolution.follow(self.redirect(&location).await?)?;
                }
                Response::Gone => {
                    return gone(request.clone(), resolution).map(
                        |observation| Located::Gone(Box::new(observation)),
                    );
                }
                Response::Issue(value) => {
                    return self.found(resolution, *value).await;
                }
            }
        }
    }

    async fn found(
        &self,
        resolution: Resolution,
        value: ReturnedIssue,
    ) -> Result<Located, ReadFailure> {
        if value.issue.pull_request.is_some() {
            return Err(ReadFailure::Unavailable);
        }
        if value.issue.number != resolution.address.number() {
            return Err(ReadFailure::IdentityConflict);
        }
        let blocked_by = self.blocked_by(&value).await?;
        let record = self.record(value, blocked_by)?;
        Ok(Located::Found(Box::new(record)))
    }
}

fn gone(
    request: Lookup,
    resolution: Resolution,
) -> Result<Observation, ReadFailure> {
    if !resolution.original() {
        return Err(ReadFailure::IdentityConflict);
    }
    let record = Unreadable {
        id: request.id,
        project: request.project,
        title: None,
        body: None,
        created: None,
        reason: ReadFailure::Deleted.to_string(),
    };
    let mut observation = Observation::invalid(record.clone());
    observation.state = IssueState::Deleted(record);
    observation.observed_at = observed_at();
    Ok(observation)
}

fn observed_at() -> Option<UnixMillis> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(UnixMillis::since_epoch)
}

pub fn request_error(error: octocrab::Error) -> ReadFailure {
    match error {
        octocrab::Error::Json { .. }
        | octocrab::Error::Serde { .. }
        | octocrab::Error::Uri { .. } => ReadFailure::InvalidResponse,
        other => read_failure(classify(other)),
    }
}

impl IssueTracker for Tracker {
    fn read(&self, request: Lookup) -> IssueCompletion<'_, Observation> {
        Box::pin(self.lookup(request))
    }

    fn list(&self, request: ListPage) -> IssueCompletion<'_, Page> {
        Box::pin(self.page(request))
    }

    fn comments(&self, issue: Lookup) -> IssueCompletion<'_, Vec<Comment>> {
        Box::pin(self.issue_comments(issue))
    }

    fn write_comment(
        &self,
        issue: Lookup,
        write: CommentWrite,
    ) -> SendCompletion<'_, Comment> {
        Box::pin(self.send(issue, write))
    }

    fn plan_relation(
        &self,
        issue: Lookup,
        change: RelationChange,
    ) -> RelationCompletion<'_> {
        Box::pin(self.relating(issue, change))
    }

    fn write_relation(
        &self,
        issue: Lookup,
        step: RelationStep,
    ) -> SendCompletion<'_, ()> {
        Box::pin(self.relate(issue, step))
    }
}
