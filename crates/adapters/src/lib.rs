use std::path::Path;

use github_api::FleetResolver;
use home::flow::snapshot::Snapshot;
use home::project::Project;
use home::tracker::{Issuer, Issuers};
use project_runtime::tracker::Connection;

type Connect =
    fn(&Path, &Snapshot, &Project, FleetResolver) -> Result<Connection, String>;

struct Adapter {
    issuer: Issuer,
    connect: Connect,
}

const ADAPTERS: &[Adapter] = &[Adapter {
    issuer: github::ISSUER,
    connect: github_api::connect,
}];

const ISSUED: [Issuer; ADAPTERS.len()] = issued();

pub const ISSUERS: Issuers = Issuers::new(&ISSUED);

const fn issued() -> [Issuer; ADAPTERS.len()] {
    let mut issued = [Issuer::MARKDOWN; ADAPTERS.len()];
    let mut at = 0;
    while at < ADAPTERS.len() {
        issued[at] = ADAPTERS[at].issuer;
        at += 1;
    }
    issued
}

pub fn connect(
    data: &Path,
    home: &Snapshot,
    project: &Project,
    resolver: FleetResolver,
) -> Result<Connection, String> {
    let adapter = project
        .backends
        .tracker
        .as_ref()
        .map_or("", |selection| selection.adapter.as_str());
    let chosen = ADAPTERS
        .iter()
        .find(|known| known.issuer.adapter() == adapter)
        .ok_or_else(|| format!("no adapter is called {adapter:?}"))?;
    (chosen.connect)(data, home, project, resolver)
}

#[cfg(test)]
mod tests;
