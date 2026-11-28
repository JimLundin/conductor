use alloc::{
    collections::BTreeMap,
    format,
    string::{String, ToString},
    vec::Vec,
};
use home::flow::Flow;
use home::issue::{Datum, body, document::targets};
use home::tracker::Ids;
use home::{IssueKey, ProjectId, operations::Handle};
use serde::Deserialize;

use super::Findings;
use crate::configuration::{Configuration, FieldSource, RelationSource};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema: u32,
    operation: Option<String>,
    #[serde(default)]
    fields: BTreeMap<String, toml::Value>,
    #[serde(default)]
    relations: BTreeMap<String, toml::Value>,
}

#[derive(Default)]
pub(super) struct Metadata {
    pub fields: BTreeMap<String, Datum>,
    pub links: BTreeMap<String, Vec<IssueKey>>,
}

#[must_use]
pub fn carries(text: &str, handle: Handle) -> bool {
    matches!(body::footer(text, "project-data"), Ok(Some((_, data)))
        if toml::from_str::<Document>(data)
            .is_ok_and(|document| document.operation == Some(handle.to_string())))
}

pub(super) fn read<'a>(
    text: &'a str,
    flow: &Flow,
    config: &Configuration,
    ids: &Ids,
    project: ProjectId,
    findings: &mut Findings,
) -> (&'a str, Metadata) {
    let (human, data) = match body::footer(text, "project-data") {
        Ok(Some(parts)) => parts,
        Ok(None) => return (text, Metadata::default()),
        Err(error) => {
            findings.error(error);
            return (text, Metadata::default());
        }
    };
    let mut metadata = Metadata::default();
    match toml::from_str::<Document>(data) {
        Ok(document) if document.schema == 1 => {
            if let Some(marker) = document.operation
                && Handle::parse(&marker)
                    .is_none_or(|handle| handle.to_string() != marker)
            {
                findings.error(String::from(
                    "invalid project-data operation handle",
                ));
            }
            for (name, value) in document.fields {
                if !config.fields.iter().any(|mapping| {
                    mapping.name == name
                        && mapping.source == FieldSource::Metadata
                }) {
                    findings.error(format!(
                        "field {name:?} has no project-data mapping"
                    ));
                    continue;
                }
                match Datum::deserialize(value) {
                    Ok(datum) => {
                        metadata.fields.insert(name, datum);
                    }
                    Err(error) => {
                        findings.error(format!("field {name:?}: {error}"));
                    }
                }
            }
            for (name, value) in document.relations {
                if !config.relations.iter().any(|mapping| {
                    mapping.name == name
                        && mapping.source == RelationSource::Metadata
                }) {
                    findings.error(format!(
                        "relation {name:?} has no project-data mapping"
                    ));
                    continue;
                }
                if let Some((relation, false)) = flow.relation(&name) {
                    match targets(relation, value, project, ids) {
                        Ok(reached) => {
                            metadata.links.insert(name, reached);
                        }
                        Err(error) => findings
                            .error(format!("relation {name:?}: {error}")),
                    }
                }
            }
        }
        Ok(document) => findings.error(format!(
            "unsupported project-data schema {}",
            document.schema
        )),
        Err(error) => {
            findings
                .error(format!("invalid project-data: {}", error.message()));
        }
    }
    (human, metadata)
}
