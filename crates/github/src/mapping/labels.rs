use alloc::{collections::BTreeMap, format, string::String, vec::Vec};
use home::flow::{Flow, Role, command::stamp};
use home::issue::{self, Datum};

use super::{Findings, Record};
use crate::configuration::{Configuration, FieldSource};

pub(super) struct Labels {
    pub managed: BTreeMap<String, String>,
    pub ordinary: Vec<String>,
}

impl Labels {
    pub fn read(
        config: &Configuration,
        names: &[String],
        findings: &mut Findings,
    ) -> Self {
        let mut managed = BTreeMap::new();
        let mut ordinary = Vec::new();
        let namespace = config.namespace.to_lowercase();
        for name in names {
            let lowercase = name.to_lowercase();
            if !lowercase.starts_with(&namespace) {
                ordinary.push(name.clone());
                continue;
            }
            let matched = config.fields.iter().find_map(|mapping| {
                let FieldSource::ManagedLabels { values } = &mapping.source
                else {
                    return None;
                };
                values
                    .iter()
                    .find(|(_, label)| label.to_lowercase() == lowercase)
                    .map(|(value, _)| (&mapping.name, value))
            });
            match matched {
                None => {
                    findings.error(format!("unknown managed label {name:?}"));
                }
                Some((field, value)) => {
                    if let Some(previous) =
                        managed.insert(field.clone(), value.clone())
                    {
                        findings.error(format!("multiple managed labels for {field:?}: {previous:?} and {value:?}"));
                    }
                }
            }
        }
        Self { managed, ordinary }
    }

    pub fn fields(
        self,
        flow: &Flow,
        kind: &str,
        config: &Configuration,
        created: u64,
        mut fields: BTreeMap<String, Datum>,
        findings: &mut Findings,
    ) -> BTreeMap<String, Datum> {
        for mapping in &config.fields {
            let Some(field) = flow.field(kind, &mapping.name) else {
                if self.managed.contains_key(&mapping.name)
                    && !matches!(
                        mapping.name.as_str(),
                        issue::KIND | issue::STAGE | issue::STATUS
                    )
                {
                    findings.error(format!(
                        "managed label for {:?} is not a field of {kind:?}",
                        mapping.name
                    ));
                }
                continue;
            };
            let mapped = match &mapping.source {
                FieldSource::CreationTime => {
                    Some(Datum::Text(creation_time(created)))
                }
                FieldSource::OrdinaryLabels => {
                    Some(Datum::Texts(self.ordinary.clone()))
                }
                FieldSource::ManagedLabels { .. } => self
                    .managed
                    .get(&field.name)
                    .cloned()
                    .map(Datum::Text)
                    .or_else(|| field.default.clone()),
                FieldSource::Metadata => None,
            };
            if let Some(value) = mapped {
                fields.insert(field.name.clone(), value);
            }
        }
        fields
    }

    pub fn status(
        &self,
        record: &Record,
        flow: &Flow,
        config: &Configuration,
        findings: &mut Findings,
    ) -> String {
        if let Some(status) = self.managed.get(home::issue::STATUS) {
            let expected =
                match flow.status(status).map(|declared| declared.role) {
                    Some(Role::Terminal) => Some("completed"),
                    Some(Role::Dropped) => Some("not_planned"),
                    _ => None,
                };
            if expected.is_none() != record.open {
                let state = if record.open { "open" } else { "closed" };
                findings.error(format!("status label {status:?} conflicts with native {state} state"));
            } else if let Some(reason) = record.state_reason.as_deref()
                && reason != expected.unwrap_or("reopened")
            {
                findings.warn(format!("native state reason {reason:?} differs from status label {status:?}"));
            }
            return status.clone();
        }
        let defaults = &config.defaults;
        match (record.open, record.state_reason.as_deref()) {
            (true, None | Some("reopened")) => defaults.open.clone(),
            (false, None) => defaults.closed.clone(),
            (false, Some("completed")) => defaults.completed.clone(),
            (false, Some("not_planned")) => defaults.not_planned.clone(),
            (_, Some(other)) => {
                findings.error(format!("unsupported native state reason {other:?} without a status label"));
                String::new()
            }
        }
    }
}

fn creation_time(milliseconds: u64) -> String {
    let to_second = stamp(milliseconds);
    let fraction = milliseconds % 1000;
    if fraction == 0 {
        to_second
    } else {
        format!("{}.{fraction:03}Z", to_second.trim_end_matches('Z'))
    }
}
