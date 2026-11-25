use std::path::Path;

use github::access::{AccessError, Observation, Read};
use home::backend::{Backend, Backends};
use octocrab::models::Repository;
use octocrab::params::repos::Commitish;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

use crate::client::classify;
use crate::{Client, HostCredentials};

const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b'/')
    .add(b'%')
    .add(b'?')
    .add(b'#')
    .add(b' ')
    .add(b'\\');

impl Client {
    async fn metadata(&self) -> Result<Repository, AccessError> {
        let metadata = self
            .reader
            .repos(self.repository.owner(), self.repository.name())
            .get()
            .await
            .map_err(classify)?;
        if !metadata
            .full_name
            .as_deref()
            .is_some_and(|name| self.repository.matches(name))
        {
            return Err(AccessError::RepositoryUnavailable);
        }
        Ok(metadata)
    }

    /// Observes read permissions at the configured repository.
    pub async fn inspect(&self, backend: Backend) -> Vec<Observation> {
        let metadata = self.metadata().await;
        let mut observations = Vec::new();
        for read in Read::for_backend(backend) {
            let result = match &metadata {
                Ok(repo) => self.probe(repo, *read).await,
                Err(error) => Err(*error),
            };
            observations.push(Observation {
                read: *read,
                result,
            });
        }
        observations
    }

    async fn probe(
        &self,
        repo: &Repository,
        read: Read,
    ) -> Result<(), AccessError> {
        let owner = self.repository.owner();
        let name = self.repository.name();
        match read {
            Read::Issues => self
                .reader
                .issues(owner, name)
                .list()
                .per_page(1u8)
                .send()
                .await
                .map(drop)
                .map_err(classify),
            Read::Workflows => self
                .reader
                .workflows(owner, name)
                .list_all_runs()
                .per_page(1u8)
                .send()
                .await
                .map(drop)
                .map_err(classify),
            Read::Statuses => self
                .reader
                .repos(owner, name)
                .list_statuses(default_branch(repo)?)
                .per_page(1u8)
                .send()
                .await
                .map(drop)
                .map_err(classify),
            Read::Checks => self.checks(repo).await,
        }
    }

    async fn checks(&self, repo: &Repository) -> Result<(), AccessError> {
        if !self.kind.permits_checks(repo.private) {
            return Err(AccessError::PermissionDenied);
        }
        self.reader
            .checks(self.repository.owner(), self.repository.name())
            .list_check_runs_for_git_ref(Commitish(default_branch(repo)?))
            .per_page(1u8)
            .send()
            .await
            .map(drop)
            .map_err(classify)
    }
}

fn default_branch(repo: &Repository) -> Result<String, AccessError> {
    let branch = repo
        .default_branch
        .as_deref()
        .ok_or(AccessError::RepositoryUnavailable)?;
    Ok(utf8_percent_encode(branch, PATH_SEGMENT).to_string())
}

/// Observes access for the project's selected GitHub tracker and host.
pub async fn access(data: &Path, backends: &Backends) -> Vec<Observation> {
    let credentials = HostCredentials::new(data);
    let mut found = Vec::new();
    for backend in [Backend::IssueTracker, Backend::RepositoryHosting] {
        if let Some(selection) = backends
            .selected(backend)
            .filter(|selection| selection.adapter == github::ADAPTER)
        {
            found.extend(
                inspect_selection(&credentials, backend, selection).await,
            );
        }
    }
    found
}

/// Checks the selected project's GitHub endpoints using this host's credentials.
///
/// # Errors
/// The home catalog is invalid or the requested project cannot be found.
pub async fn project_access(
    catalog: &Path,
    project: &str,
    data: &Path,
) -> Result<(String, u8), String> {
    let configuration = home_files::configuration(catalog);
    let selected = github::configuration::selected(&configuration, project)?;
    let observed = access(data, &selected.backends).await;
    let code = u8::from(observed.iter().any(|read| read.result.is_err()));
    Ok((github::access::report(&observed), code))
}

async fn inspect_selection(
    credentials: &HostCredentials,
    backend: Backend,
    selection: &home::backend::Selection,
) -> Vec<Observation> {
    match credentials.connect(selection) {
        Ok(client) => client.inspect(backend).await,
        Err(error) => Read::for_backend(backend)
            .iter()
            .map(|read| Observation {
                read: *read,
                result: Err(error),
            })
            .collect(),
    }
}
