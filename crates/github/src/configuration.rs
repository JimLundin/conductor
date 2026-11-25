use alloc::format;
use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    vec::Vec,
};
use home::backend::{Backend, Selection};
use home::flow::Flow;
use serde::{Deserialize, Serialize};

mod validation;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    pub open: String,
    pub completed: String,
    pub not_planned: String,
    pub closed: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FieldSource {
    ManagedLabels { values: BTreeMap<String, String> },
    CreationTime,
    OrdinaryLabels,
    Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FieldMapping {
    pub name: String,
    pub source: FieldSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationSource {
    Parent,
    Dependency,
    Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RelationMapping {
    pub name: String,
    pub source: RelationSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configuration {
    pub namespace: String,
    pub defaults: Defaults,
    pub fields: Vec<FieldMapping>,
    pub relations: Vec<RelationMapping>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct File {
    namespace: Option<String>,
    defaults: Option<Defaults>,
    fields: Vec<FieldMapping>,
    relations: Vec<RelationMapping>,
}

/// # Errors
/// The options are malformed or cannot map the declared flow consistently.
pub fn load(text: &str, flow: &Flow) -> Result<Configuration, String> {
    let file: File =
        toml::from_str(text).map_err(|error| error.message().to_string())?;
    validation::configuration(file, flow)
}

#[cfg(test)]
mod tests;

/// Finds a selected project and validates its configuration before contacting GitHub.
///
/// # Errors
/// The project is unknown or its configuration cannot map the issue flow.
pub fn selected<'a>(
    configuration: &'a home::flow::snapshot::Snapshot,
    name: &str,
) -> Result<&'a home::project::Project, String> {
    let project = configuration.project(name).ok_or_else(|| {
        configuration
            .listing_problems()
            .first()
            .cloned()
            .unwrap_or_else(|| format!("unknown project {name:?}"))
    })?;
    project
        .backends
        .validate()
        .map_err(|refusal| refusal.label())?;
    [
        Backend::VersionControl,
        Backend::IssueTracker,
        Backend::RepositoryHosting,
    ]
    .into_iter()
    .filter_map(|backend| {
        project
            .backends
            .selected(backend)
            .map(|selection| (backend, selection))
    })
    .try_for_each(|(backend, selection)| {
        validate(backend, selection, configuration.flow())
    })?;
    Ok(project)
}

fn validate(
    backend: Backend,
    selection: &Selection,
    flow: Result<&Flow, &str>,
) -> Result<(), String> {
    match (backend, selection.adapter.as_str()) {
        (Backend::IssueTracker, crate::ADAPTER) => {
            load(&selection.options, flow?).map(drop)
        }
        _ => Ok(()),
    }
}
