use github::access::routes::Address;
use home::issue::{IssueWrite, Lookup, NativeIssue, ReadFailure, SendFailure};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::de::IgnoredAny;

use crate::Tracker;
use crate::client::unsent;
use crate::tracking::{Response, native};

#[derive(serde::Serialize)]
struct Labels<'a> {
    labels: [&'a str; 1],
}

#[derive(serde::Serialize)]
#[serde(untagged)]
enum Patch<'a> {
    Title {
        title: &'a str,
    },
    Body {
        body: &'a str,
    },
    State {
        state: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        state_reason: Option<&'a str>,
    },
}

impl Tracker {
    pub(crate) async fn native_issue(
        &self,
        issue: Lookup,
    ) -> Result<NativeIssue, ReadFailure> {
        let address = Address::of(&issue.id)?;
        match self.fetch(address).await? {
            Response::Redirect(_) => Err(ReadFailure::IdentityConflict),
            Response::Gone => Err(ReadFailure::Deleted),
            Response::Issue(returned) => {
                let client = &self.client;
                let found = returned.issue;
                address.returned(found.number, found.pull_request.is_some())?;
                client.repository.verify_record(
                    &client.endpoint,
                    found.repository_url.as_str(),
                    found.url.as_str(),
                    found.number,
                )?;
                Ok(native(found, returned.state_reason))
            }
        }
    }

    pub(crate) async fn apply(
        &self,
        issue: Lookup,
        write: IssueWrite,
    ) -> Result<(), SendFailure> {
        let route = Address::of(&issue.id)
            .map_err(SendFailure::Rejected)?
            .route(&self.client.repository);
        let reader = &self.client.reader;
        let sent: Result<IgnoredAny, octocrab::Error> = match &write {
            IssueWrite::AddLabel(name) => {
                reader
                    .post(
                        format!("{route}/labels"),
                        Some(&Labels { labels: [name] }),
                    )
                    .await
            }
            IssueWrite::RemoveLabel(name) => {
                let label = utf8_percent_encode(name, NON_ALPHANUMERIC);
                reader
                    .delete(format!("{route}/labels/{label}"), None::<&()>)
                    .await
            }
            IssueWrite::Title { to, .. } => {
                reader.patch(route, Some(&Patch::Title { title: to })).await
            }
            IssueWrite::Body { to, .. } => {
                reader.patch(route, Some(&Patch::Body { body: to })).await
            }
            IssueWrite::Close { reason, .. } => {
                let state = Patch::State {
                    state: "closed",
                    state_reason: Some(reason),
                };
                reader.patch(route, Some(&state)).await
            }
            IssueWrite::Reopen { .. } => {
                let state = Patch::State {
                    state: "open",
                    state_reason: None,
                };
                reader.patch(route, Some(&state)).await
            }
        };
        sent.map(drop).map_err(unsent)
    }
}
