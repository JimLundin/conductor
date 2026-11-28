use std::path::{Path, PathBuf};

use home::backend::IssueQuery;
use home::issue::IssueListing;

use crate::resolve::Trackers;

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
    let loaded = home_files::read(root, adapters::ISSUERS);
    let project = loaded
        .snapshot
        .project(query.project)
        .ok_or_else(|| {
            loaded
                .snapshot
                .listing_problems()
                .first()
                .cloned()
                .unwrap_or_else(|| {
                    format!("unknown project {:?}", query.project)
                })
        })?
        .id;
    let data = query.data.map(PathBuf::from);
    let trackers = Trackers::new(root, loaded, data, crate::resolve::connect)?;
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
