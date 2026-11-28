use alloc::format;
use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    vec::Vec,
};
use home::backend::Selection;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configuration {
    pub namespace: String,
    pub defaults: Defaults,
    pub fields: Vec<FieldMapping>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct File {
    namespace: Option<String>,
    defaults: Option<Defaults>,
    fields: Vec<FieldMapping>,
}

pub fn load(text: &str, flow: &Flow) -> Result<Configuration, String> {
    let file: File =
        toml::from_str(text).map_err(|error| error.message().to_string())?;
    validation::configuration(file, flow)
}

#[cfg(test)]
mod tests;

pub struct Selected<'a> {
    pub project: &'a home::project::Project,
    pub tracker: Option<TrackerMapping<'a>>,
}

pub struct TrackerMapping<'a> {
    pub selection: &'a Selection,
    pub flow: &'a Flow,
    pub mapping: Configuration,
}

pub fn selected<'a>(
    configuration: &'a home::flow::snapshot::Snapshot,
    name: &str,
) -> Result<Selected<'a>, String> {
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
    let tracker = project
        .backends
        .tracker
        .as_ref()
        .filter(|selection| selection.adapter == crate::ADAPTER)
        .map(|selection| {
            let flow = configuration.flow()?;
            let mapping = load(&selection.options, flow)?;
            Ok::<_, String>(TrackerMapping {
                selection,
                flow,
                mapping,
            })
        })
        .transpose()?;
    Ok(Selected { project, tracker })
}
