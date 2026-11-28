use alloc::{
    collections::BTreeMap,
    format,
    string::{String, ToString},
    vec::Vec,
};
use home::flow::{Flow, Relation};
use home::issue::{
    Datum, body,
    document::{relation_value, targets},
};
use home::tracker::Ids;
use home::{IssueKey, ProjectId, operations::Handle};
use serde::Deserialize;

use super::Findings;
use crate::configuration::{Configuration, FieldSource};

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
                let Some((relation, false)) = flow.relation(&name) else {
                    findings.error(format!("no relation is called {name:?}"));
                    continue;
                };
                match targets(relation, value, project, ids) {
                    Ok(reached) => {
                        metadata.links.insert(name, reached);
                    }
                    Err(error) => {
                        findings.error(format!("relation {name:?}: {error}"));
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

pub(super) fn relate(
    text: &str,
    relation: &Relation,
    project: ProjectId,
    ids: &Ids,
    wanted: &[IssueKey],
) -> Result<Option<String>, String> {
    let (human, mut document) = match body::footer(text, "project-data")? {
        Some((human, data)) => (
            human,
            toml::from_str::<toml::Table>(data)
                .map_err(|error| String::from(error.message()))?,
        ),
        None => (text, toml::Table::new()),
    };
    let mut relations = match document.remove("relations") {
        Some(toml::Value::Table(table)) => table,
        Some(_) => return Err(String::from("relations is not a table")),
        None => toml::Table::new(),
    };
    let current = relations
        .get(&relation.name)
        .cloned()
        .map(|value| targets(relation, value, project, ids))
        .transpose()?
        .unwrap_or_default();
    if current == wanted {
        return Ok(None);
    }
    if wanted.is_empty() {
        relations.remove(&relation.name);
    } else {
        relations.insert(
            relation.name.clone(),
            relation_value(relation.many, wanted, project),
        );
    }
    document.entry("schema").or_insert(toml::Value::Integer(1));
    if !relations.is_empty() {
        document
            .insert(String::from("relations"), toml::Value::Table(relations));
    }
    let kept = human.trim_end();
    if document.keys().all(|key| key == "schema") {
        return Ok(Some(String::from(kept)));
    }
    let data = toml::to_string(&document).map_err(|error| error.to_string())?;
    let gap = if kept.is_empty() { "" } else { "\n\n" };
    Ok(Some(format!("{kept}{gap}```project-data\n{data}```\n")))
}
