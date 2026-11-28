use home::IssueId;
use home::UnixMillis;
use home::issue::{IssueCreate, Opened, Openings, ReadFailure, SendFailure};
use home::operations::Handle;

use crate::Tracker;
use crate::client::unsent;
use crate::tracking::{ReturnedIssue, native};

impl Tracker {
    pub(crate) async fn open(
        &self,
        create: IssueCreate,
    ) -> Result<IssueId, SendFailure> {
        let route = self.client.repository.issues_route();
        let sent: ReturnedIssue = self
            .client
            .reader
            .post(route, Some(&create))
            .await
            .map_err(unsent)?;
        self.identified(&sent).map_err(|_| SendFailure::Unknown)
    }

    pub(crate) async fn carrying(
        &self,
        handle: Handle,
    ) -> Result<Openings, ReadFailure> {
        let mut openings = Openings::default();
        let mut continuation = None;
        loop {
            let (items, next) = self.raw_page(continuation.as_deref()).await?;
            let newest = items
                .iter()
                .filter_map(|returned| {
                    u64::try_from(returned.issue.created_at.timestamp_millis())
                        .ok()
                })
                .max()
                .map(UnixMillis);
            openings.newest = openings.newest.max(newest);
            openings.found.extend(self.marked(items, handle)?);
            continuation = next;
            if continuation.is_none() {
                return Ok(openings);
            }
        }
    }

    fn marked(
        &self,
        items: Vec<ReturnedIssue>,
        handle: Handle,
    ) -> Result<Vec<Opened>, ReadFailure> {
        items
            .into_iter()
            .filter(|returned| {
                returned
                    .issue
                    .body
                    .as_deref()
                    .is_some_and(|body| github::mapping::carries(body, handle))
            })
            .map(|returned| {
                Ok(Opened {
                    id: self.identified(&returned)?,
                    native: native(returned.issue, returned.state_reason),
                })
            })
            .collect()
    }

    fn identified(
        &self,
        returned: &ReturnedIssue,
    ) -> Result<IssueId, ReadFailure> {
        let client = &self.client;
        let issue = &returned.issue;
        client.repository.verify_record(
            &client.endpoint,
            issue.repository_url.as_str(),
            issue.url.as_str(),
            issue.number,
        )?;
        github::IssueNumber::new(issue.number)
            .map(|number| IssueId::of(&number))
            .ok_or(ReadFailure::InvalidResponse)
    }
}
