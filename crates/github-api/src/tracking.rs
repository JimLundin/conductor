use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use github::access::routes::{Address, Resolution};
use github::configuration::Configuration;
use github::mapping::{self, Record};
use home::flow::Flow;
use home::issue::{Comment, CommentWrite};
use home::issue::{
    IssueState, ListPage, Locator, Lookup, Observation, Page, ReadFailure,
    Unreadable,
};
use home::{IssueId, ProjectId, UnixMillis};
use octocrab::{FromResponse, models::issues::Issue, params};
use project_runtime::{IssueCompletion, IssueTracker, SendCompletion};

use crate::{Client, client::classify, read_failure};

pub struct Tracker {
    pub(crate) client: Client,
    flow: Flow,
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
struct ReturnedIssue {
    state_reason: Option<String>,
    #[serde(flatten)]
    issue: Issue,
}

enum Response {
    Issue(Box<ReturnedIssue>),
    Redirect(String),
    Gone,
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
        let records = raw
            .items
            .into_iter()
            .filter(|returned| returned.issue.pull_request.is_none())
            .map(|returned| self.observe(request.project, returned))
            .collect::<Result<_, _>>()?;
        Ok(Page { records, next })
    }

    fn observe(
        &self,
        project: ProjectId,
        value: ReturnedIssue,
    ) -> Result<Observation, ReadFailure> {
        let client = &self.client;
        let issue = value.issue;
        client.repository.verify_record(
            &client.endpoint,
            issue.repository_url.as_str(),
            issue.url.as_str(),
            issue.number,
        )?;
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
        let record = Record {
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
        };
        let mut observation =
            mapping::observe(&self.flow, &self.config, project, record);
        observation.observed_at = observed_at();
        Ok(observation)
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
        let mut resolution = Resolution::start(Address::of(&request.id)?);
        loop {
            match self.fetch(resolution.address).await? {
                Response::Redirect(location) => {
                    resolution =
                        resolution.follow(self.redirect(&location).await?)?;
                }
                Response::Gone => return gone(request, resolution),
                Response::Issue(issue) => {
                    return self.resolved(&request, resolution, *issue);
                }
            }
        }
    }

    fn resolved(
        &self,
        request: &Lookup,
        resolution: Resolution,
        value: ReturnedIssue,
    ) -> Result<Observation, ReadFailure> {
        if value.issue.pull_request.is_some() {
            return Err(ReadFailure::Unavailable);
        }
        if value.issue.number != resolution.address.number() {
            return Err(ReadFailure::IdentityConflict);
        }
        let observed = self.observe(request.project, value)?;
        home::issue::cache::verify(request, &observed)?;
        Ok(observed)
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
}
