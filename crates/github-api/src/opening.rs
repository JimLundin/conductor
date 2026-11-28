use home::IssueId;
use home::issue::{IssueCreate, Opened, ReadFailure, SendFailure};
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
    ) -> Result<Vec<Opened>, ReadFailure> {
        let mut found = Vec::new();
        let mut continuation = None;
        loop {
            let (items, next) = self.raw_page(continuation.as_deref()).await?;
            found.extend(self.marked(items, handle)?);
            continuation = next;
            if continuation.is_none() {
                return Ok(found);
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
        IssueId::new(&issue.number.to_string())
            .ok_or(ReadFailure::InvalidResponse)
    }
}
