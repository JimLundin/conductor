use std::path::{Path, PathBuf};
use std::sync::Arc;

use github::configuration::{TrackerMapping, selected};
use home::backend::{Capability, CredentialState, IssueQuery, Refusal};
use home::issue::{ReadFailure, ReadMode};
use project_runtime::issues::IssueCache;
use project_runtime::{Registration, Registry};

pub fn register(
    registry: &mut Registry,
    data: &Path,
    tracker: TrackerMapping<'_>,
) -> Result<(), ReadFailure> {
    let client = github_api::HostCredentials::new(data)
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
        Ok(query) => super::project::run(read(root, query)),
        Err(usage) => (format!("usage: {usage}\n"), 2),
    })
}

async fn read(
    root: &Path,
    query: IssueQuery<'_>,
) -> Result<(String, u8), String> {
    let configuration = home_files::configuration(root);
    let selected = selected(&configuration, query.project)?;
    selected
        .project
        .backends
        .tracker
        .as_ref()
        .ok_or_else(|| Refusal::TrackerNotConfigured.label())?;
    let data =
        project_runtime::data_dir_or_default(query.data.map(PathBuf::from))
            .map_err(|error| error.to_string())?;
    let cache = IssueCache::new(&data, configuration.flow()?);
    let mut registry = Registry::default();
    let mode = mode(&mut registry, &data, selected.tracker, query.offline);
    let service = registry.project(&selected.project.backends);
    let id = selected.project.id;
    let (json, found, failure) = match query.reference {
        Some(reference) => {
            let read = service
                .issue(&cache, id, reference, mode)
                .await
                .map_err(|err| err.to_string())?;
            (
                serde_json::to_string_pretty(&read),
                read.record.is_some(),
                read.status.failure,
            )
        }
        None => {
            let list = service
                .issues(&cache, id, mode)
                .await
                .map_err(|err| err.to_string())?;
            (
                serde_json::to_string_pretty(&list),
                list.status.complete,
                list.status.failure,
            )
        }
    };
    Ok((
        format!("{}\n", json.map_err(|err| err.to_string())?),
        u8::from(!found || failure.is_some()),
    ))
}

fn mode(
    registry: &mut Registry,
    data: &Path,
    mapping: Option<TrackerMapping<'_>>,
    offline: bool,
) -> ReadMode {
    if offline {
        return ReadMode::Cached;
    }
    mapping
        .map_or(Ok(()), |tracker| register(registry, data, tracker))
        .map_or_else(ReadMode::Failed, |()| ReadMode::Refresh)
}
