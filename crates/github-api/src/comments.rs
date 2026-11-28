use github::access::routes::Address;
use github::log::Posted;
use home::backend::Capability;
use home::issue::{
    Comment, CommentWrite, IssueComment, Lookup, ReadFailure, SendFailure,
};
use home::{IssueId, UnixMillis};
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
        self.listed(number)
            .await?
            .into_iter()
            .map(observe)
            .collect()
    }

    pub(crate) async fn posted(
        &self,
        number: u64,
    ) -> Result<Vec<Posted>, ReadFailure> {
        self.listed(number)
            .await?
            .into_iter()
            .map(|comment| {
                Ok(Posted {
                    id: comment.id.to_string(),
                    author: comment.user.login.clone(),
                    created: millis(comment.created_at.timestamp_millis())?,
                    body: comment.body.unwrap_or_default(),
                })
            })
            .collect()
    }

    async fn listed(
        &self,
        number: u64,
    ) -> Result<Vec<issues::Comment>, ReadFailure> {
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
            found.extend(listed.items);
            if !more {
                return Ok(found);
            }
            page = page.checked_add(1).ok_or(ReadFailure::InvalidResponse)?;
        }
    }

    pub(crate) async fn marked_comments(
        &self,
    ) -> Result<Vec<IssueComment>, ReadFailure> {
        let repository = &self.client.repository;
        let issues = self
            .client
            .reader
            .issues(repository.owner(), repository.name());
        let mut found = Vec::new();
        let mut page = 1u32;
        loop {
            let listed = issues
                .list_issue_comments()
                .per_page(100u8)
                .page(page)
                .send()
                .await
                .map_err(request_error)?;
            let more = listed.next.is_some();
            let items = listed
                .items
                .into_iter()
                .map(|comment| {
                    Ok(IssueComment {
                        issue: commented(&comment)?,
                        comment: observe(comment)?,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            found.extend(github::canvas::marked(items));
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

fn commented(comment: &issues::Comment) -> Result<IssueId, ReadFailure> {
    comment
        .issue_url
        .as_ref()
        .and_then(|url| url.path_segments()?.next_back())
        .and_then(|number| github::ISSUER.read(number))
        .ok_or(ReadFailure::InvalidResponse)
}

fn observe(comment: issues::Comment) -> Result<Comment, ReadFailure> {
    Ok(Comment {
        id: comment.id.to_string(),
        url: comment.html_url.to_string(),
        created: millis(comment.created_at.timestamp_millis())?,
        updated: millis(
            comment
                .updated_at
                .unwrap_or(comment.created_at)
                .timestamp_millis(),
        )?,
        body: comment.body.unwrap_or_default(),
    })
}

fn millis(at: i64) -> Result<UnixMillis, ReadFailure> {
    u64::try_from(at)
        .map(UnixMillis)
        .map_err(|_| ReadFailure::InvalidResponse)
}
