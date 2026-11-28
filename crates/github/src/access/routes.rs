use crate::IssueNumber;
use alloc::{
    format,
    string::{String, ToString},
};
use home::issue::ReadFailure;
use home::{IssueId, TrackerId};

use super::Repository;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Address {
    Named(u64),
    RepositoryId { repository: u64, issue: u64 },
}

impl Address {
    pub fn of(id: &IssueId) -> Result<Self, ReadFailure> {
        id.tracked::<IssueNumber>()
            .map(|named| Self::Named(named.get()))
            .ok_or(ReadFailure::InvalidResponse)
    }

    pub const fn returned(
        self,
        number: u64,
        pull_request: bool,
    ) -> Result<(), ReadFailure> {
        if pull_request {
            Err(ReadFailure::Unavailable)
        } else if number == self.number() {
            Ok(())
        } else {
            Err(ReadFailure::IdentityConflict)
        }
    }

    #[must_use]
    pub const fn number(self) -> u64 {
        match self {
            Self::Named(number) | Self::RepositoryId { issue: number, .. } => {
                number
            }
        }
    }

    #[must_use]
    pub fn route(self, repo: &Repository) -> String {
        match self {
            Self::Named(number) => format!("{}/{number}", repo.issues_route()),
            Self::RepositoryId { repository, issue } => {
                format!("/repositories/{repository}/issues/{issue}")
            }
        }
    }
}

fn number(text: &str) -> Option<u64> {
    IssueNumber::read(text).map(IssueNumber::get)
}

fn scoped_path<'a>(endpoint: &str, url: &'a str) -> Option<&'a str> {
    let path = url.strip_prefix(endpoint).unwrap_or(url);
    path.strip_prefix('/')
}

enum Collection {
    Named,
    Id(u64),
}

impl Repository {
    #[must_use]
    pub fn issues_route(&self) -> String {
        format!("/repos/{}/{}/issues", self.owner(), self.name())
    }

    fn collection(&self, path: &str) -> Option<Collection> {
        let resource = path.strip_suffix("/issues")?;
        if let Some(id) = resource.strip_prefix("repositories/") {
            return number(id).map(Collection::Id);
        }
        self.matches(resource.strip_prefix("repos/")?)
            .then_some(Collection::Named)
    }

    pub fn redirect(
        &self,
        endpoint: &str,
        location: &str,
    ) -> Result<Address, ReadFailure> {
        let (collection, number_text) = scoped_path(endpoint, location)
            .and_then(|path| path.rsplit_once('/'))
            .ok_or(ReadFailure::IdentityConflict)?;
        let issue = number(number_text).ok_or(ReadFailure::IdentityConflict)?;
        let destination = self
            .collection(collection)
            .ok_or(ReadFailure::IdentityConflict)?;
        Ok(match destination {
            Collection::Named => Address::Named(issue),
            Collection::Id(repository) => {
                Address::RepositoryId { repository, issue }
            }
        })
    }

    pub fn verify_record(
        &self,
        endpoint: &str,
        repo_url: &str,
        issue_url: &str,
        number: u64,
    ) -> Result<(), ReadFailure> {
        let resource = repo_url
            .strip_prefix(endpoint)
            .and_then(|path| path.strip_prefix("/repos/"))
            .ok_or(ReadFailure::IdentityConflict)?;
        if !self.matches(resource)
            || self.redirect(endpoint, issue_url)? != Address::Named(number)
        {
            return Err(ReadFailure::IdentityConflict);
        }
        Ok(())
    }

    pub fn continuation(
        &self,
        endpoint: &str,
        page: u32,
        next: Option<&str>,
        incomplete: Option<bool>,
    ) -> Result<Option<String>, ReadFailure> {
        if incomplete == Some(true) {
            return Err(ReadFailure::InvalidResponse);
        }
        let Some(url) = next else {
            return Ok(None);
        };
        let (path, query) = scoped_path(endpoint, url)
            .and_then(|path| path.split_once('?'))
            .ok_or(ReadFailure::InvalidResponse)?;
        self.collection(path).ok_or(ReadFailure::InvalidResponse)?;
        let mut pages = query
            .split('&')
            .filter_map(|param| param.strip_prefix("page="));
        let next_page =
            page.checked_add(1).ok_or(ReadFailure::InvalidResponse)?;
        if pages.next().and_then(number) != Some(u64::from(next_page))
            || pages.next().is_some()
        {
            return Err(ReadFailure::InvalidResponse);
        }
        Ok(Some(next_page.to_string()))
    }
}

const MAX_REDIRECTS: usize = 8;

#[derive(Clone, Copy)]
pub struct Resolution {
    pub address: Address,
    redirects: usize,
}

impl Resolution {
    #[must_use]
    pub const fn start(address: Address) -> Self {
        Self {
            address,
            redirects: 0,
        }
    }

    pub const fn follow(self, address: Address) -> Result<Self, ReadFailure> {
        if self.redirects >= MAX_REDIRECTS {
            return Err(ReadFailure::InvalidResponse);
        }
        Ok(Self {
            address,
            redirects: self.redirects + 1,
        })
    }

    #[must_use]
    pub const fn original(self) -> bool {
        self.redirects == 0
    }
}
