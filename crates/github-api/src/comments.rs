use github::access::routes::Address;
use home::UnixMillis;
use home::backend::Capability;
use home::issue::{Comment, CommentWrite, Lookup, ReadFailure, SendFailure};
use octocrab::models::{CommentId, issues};

use crate::Tracker;
use crate::client::unsent;
use crate::tracking::request_error;

#[derive(serde::Serialize)]
struct Edit<'a> {
    body: &'a str,
}

impl Tracker {
    #[must_use]
    pub fn capabilities(&self) -> Vec<Capability> {
        std::iter::once(Capability::ReadIssues)
            .chain(self.client.writes.then_some(Capability::WriteIssues))
            .collect()
    }

    pub(crate) async fn issue_comments(
        &self,
        issue: Lookup,
    ) -> Result<Vec<Comment>, ReadFailure> {
        let number = Address::of(&issue.id)?.number();
        let repository = &self.client.repository;
        let issues = self
            .client
            .reader
            .issues(repository.owner(), repository.name());
        let mut found = Vec::new();
        let mut page = 1u32;
        loop {
            let listed = issues
                .list_comments(number)
                .per_page(100u8)
                .page(page)
                .send()
                .await
                .map_err(request_error)?;
            let more = listed.next.is_some();
            let items = listed
                .items
                .into_iter()
                .map(observe)
                .collect::<Result<Vec<_>, _>>()?;
            found.extend(items);
            if !more {
                return Ok(found);
            }
            page = page.checked_add(1).ok_or(ReadFailure::InvalidResponse)?;
        }
    }

    pub(crate) async fn send(
        &self,
        issue: Lookup,
        write: CommentWrite,
    ) -> Result<Comment, SendFailure> {
        let number = Address::of(&issue.id)
            .map_err(SendFailure::Rejected)?
            .number();
        let repository = &self.client.repository;
        let reader = &self.client.reader;
        let sent = match write {
            CommentWrite::Create { body } => {
                reader
                    .issues(repository.owner(), repository.name())
                    .create_comment(number, body)
                    .await
            }
            CommentWrite::Edit { comment, body } => {
                let id =
                    comment.parse::<u64>().map(CommentId).map_err(|_| {
                        SendFailure::Rejected(ReadFailure::InvalidConfiguration)
                    })?;
                let route =
                    format!("{}/comments/{id}", repository.issues_route());
                reader.patch(route, Some(&Edit { body: &body })).await
            }
        };
        observe(sent.map_err(unsent)?).map_err(|_| SendFailure::Unknown)
    }
}

fn observe(comment: issues::Comment) -> Result<Comment, ReadFailure> {
    let updated = comment.updated_at.unwrap_or(comment.created_at);
    Ok(Comment {
        id: comment.id.to_string(),
        url: comment.html_url.to_string(),
        body: comment.body.unwrap_or_default(),
        updated: u64::try_from(updated.timestamp_millis())
            .map(UnixMillis)
            .map_err(|_| ReadFailure::InvalidResponse)?,
    })
}
