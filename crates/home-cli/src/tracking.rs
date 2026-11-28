use std::path::{Path, PathBuf};
use std::sync::Arc;

use github::configuration::{TrackerMapping, selected};
use home::backend::{Capability, CredentialState, IssueQuery};
use home::issue::{IssueListing, ReadFailure};
use project_runtime::{Registration, Registry};

use crate::resolve::Trackers;

pub fn register(
    registry: &mut Registry,
    data: &Path,
    tracker: TrackerMapping<'_>,
) -> Result<(), ReadFailure> {
    let client = github_api::HostCredentials::with_fleet(
        data,
        daemon::fleet::project_credential,
    )
    .connect(tracker.selection)
    .map_err(github_api::read_failure)?;
    let reader = client.tracker(tracker.flow.clone(), tracker.mapping);
    registry
        .register_tracker(
            Registration {
                selection: tracker.selection.clone(),
                capabilities: vec![Capability::ReadIssues],
                credential: CredentialState::Available,
            },
            Arc::new(reader),
        )
        .map_err(ReadFailure::Refused)
}

pub fn answer(root: &Path, args: &[String]) -> Option<(String, u8)> {
    let parsed = home::backend::issue_query(args)?;
    Some(match parsed {
        Ok(query) => super::project::run(listed(root, query)),
        Err(usage) => (format!("usage: {usage}\n"), 2),
    })
}

async fn listed(
    root: &Path,
    query: IssueQuery<'_>,
) -> Result<(String, u8), String> {
    let loaded = home_files::read(root);
    let project = selected(&loaded.snapshot, query.project)?.project.id;
    let data = query.data.map(PathBuf::from);
    let mut trackers = Trackers::new(root, loaded, data);
    shown(&trackers.listing(project, query.offline).await?)
}

pub fn shown(listing: &IssueListing) -> Result<(String, u8), String> {
    let json =
        serde_json::to_string_pretty(listing).map_err(|err| err.to_string())?;
    Ok((
        format!("{json}\n"),
        u8::from(!listing.status.complete || listing.status.failure.is_some()),
    ))
}
