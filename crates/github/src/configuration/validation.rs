use super::{
    Configuration, Defaults, FieldMapping, FieldSource, File, RelationMapping,
    RelationSource,
};
use alloc::{
    collections::{BTreeMap, BTreeSet},
    format,
    string::String,
    vec::Vec,
};
use home::flow::{FieldRole, FieldType, Flow, Role};

pub(super) fn configuration(
    file: File,
    flow: &Flow,
) -> Result<Configuration, String> {
    let namespace = file.namespace.unwrap_or_else(|| String::from("work/"));
    if namespace.len() < 2
        || !namespace.ends_with('/')
        || namespace.chars().any(char::is_whitespace)
        || namespace.chars().any(char::is_control)
    {
        return Err(String::from(
            "the managed-label namespace must be nonempty, end in / and contain no whitespace or controls",
        ));
    }
    let completed = status(flow, Role::Terminal)?;
    let defaults = file.defaults.unwrap_or(Defaults {
        open: status(flow, Role::Idle)?,
        completed: completed.clone(),
        not_planned: status(flow, Role::Dropped)?,
        closed: completed,
    });
    check_defaults(&defaults, flow)?;
    let fields = fields(flow, &namespace, file.fields)?;
    check_labels(&namespace, &fields)?;
    let relations = relations(flow, file.relations)?;
    Ok(Configuration {
        namespace,
        defaults,
        fields,
        relations,
    })
}

fn status(flow: &Flow, role: Role) -> Result<String, String> {
    flow.statuses
        .iter()
        .find(|status| status.role == role)
        .map(|status| status.name.clone())
        .ok_or_else(|| format!("the flow has no {} status", role.word()))
}

fn check_defaults(defaults: &Defaults, flow: &Flow) -> Result<(), String> {
    for (name, value) in [
        ("open", &defaults.open),
        ("completed", &defaults.completed),
        ("not_planned", &defaults.not_planned),
        ("closed", &defaults.closed),
    ] {
        let role = flow
            .statuses
            .iter()
            .find(|status| &status.name == value)
            .map(|status| status.role)
            .ok_or_else(|| {
                format!("default {name}: no status is called {value:?}")
            })?;
        let allowed = match name {
            "open" => !matches!(role, Role::Terminal | Role::Dropped),
            "completed" => role == Role::Terminal,
            "not_planned" => role == Role::Dropped,
            _ => matches!(role, Role::Terminal | Role::Dropped),
        };
        if !allowed {
            return Err(format!(
                "default {name}: status {value:?} has an incompatible role"
            ));
        }
    }
    Ok(())
}

fn values(flow: &Flow, name: &str) -> Option<BTreeSet<String>> {
    match name {
        "kind" => {
            Some(flow.kinds.iter().map(|kind| kind.name.clone()).collect())
        }
        "stage" => {
            Some(flow.stages.iter().map(|stage| stage.name.clone()).collect())
        }
        "status" => Some(
            flow.statuses
                .iter()
                .map(|status| status.name.clone())
                .collect(),
        ),
        _ => {
            let fields: Vec<_> = flow
                .fields
                .iter()
                .chain(flow.kinds.iter().flat_map(|kind| &kind.fields))
                .filter(|field| field.name == name)
                .collect();
            if fields.is_empty() {
                return None;
            }
            let mut all = BTreeSet::new();
            for field in fields {
                let FieldType::Enum(values) = &field.value else {
                    return None;
                };
                all.extend(values.iter().cloned());
            }
            Some(all)
        }
    }
}

fn labels(
    namespace: &str,
    name: &str,
    values: BTreeSet<String>,
) -> FieldSource {
    FieldSource::ManagedLabels {
        values: values
            .into_iter()
            .map(|value| {
                let label = format!("{namespace}{name}/{value}");
                (value, label)
            })
            .collect(),
    }
}

fn fields(
    flow: &Flow,
    namespace: &str,
    overrides: Vec<FieldMapping>,
) -> Result<Vec<FieldMapping>, String> {
    let mut fields = BTreeMap::new();
    for name in ["kind", "stage", "status"] {
        fields.insert(
            String::from(name),
            labels(namespace, name, values(flow, name).unwrap_or_default()),
        );
    }
    for field in flow
        .fields
        .iter()
        .chain(flow.kinds.iter().flat_map(|kind| &kind.fields))
    {
        let source = match field.role {
            Some(FieldRole::Created) => FieldSource::CreationTime,
            Some(FieldRole::Labels) => FieldSource::OrdinaryLabels,
            _ if field.name == "priority" => values(flow, &field.name)
                .map_or(FieldSource::Metadata, |values| {
                    labels(namespace, &field.name, values)
                }),
            _ => FieldSource::Metadata,
        };
        fields.entry(field.name.clone()).or_insert(source);
    }
    let mut seen = BTreeSet::new();
    for field in overrides {
        if !seen.insert(field.name.clone()) {
            return Err(format!(
                "field {:?} is mapped more than once",
                field.name
            ));
        }
        let original = fields
            .get(&field.name)
            .ok_or_else(|| format!("no field is called {:?}", field.name))?;
        check_source(flow, &field, original)?;
        fields.insert(field.name, field.source);
    }
    Ok(fields
        .into_iter()
        .map(|(name, source)| FieldMapping { name, source })
        .collect())
}

fn check_source(
    flow: &Flow,
    field: &FieldMapping,
    original: &FieldSource,
) -> Result<(), String> {
    let supported = match original {
        FieldSource::CreationTime => field.source == FieldSource::CreationTime,
        FieldSource::OrdinaryLabels => {
            field.source == FieldSource::OrdinaryLabels
        }
        FieldSource::ManagedLabels { .. } => {
            matches!(field.source, FieldSource::ManagedLabels { .. })
        }
        FieldSource::Metadata => matches!(
            field.source,
            FieldSource::Metadata | FieldSource::ManagedLabels { .. }
        ),
    };
    if !supported {
        return Err(format!(
            "field {:?} cannot replace its native source",
            field.name
        ));
    }
    if let FieldSource::ManagedLabels { values: mapped } = &field.source {
        let declared = values(flow, &field.name)
            .ok_or_else(|| format!("field {:?} is not an enum", field.name))?;
        let given: BTreeSet<String> = mapped.keys().cloned().collect();
        if given != declared {
            return Err(format!(
                "field {:?} must map every declared value exactly once, with no undeclared values",
                field.name
            ));
        }
    }
    Ok(())
}

fn check_labels(
    namespace: &str,
    fields: &[FieldMapping],
) -> Result<(), String> {
    let mut seen = BTreeMap::new();
    let reserved = namespace.to_lowercase();
    for field in fields {
        let FieldSource::ManagedLabels { values } = &field.source else {
            continue;
        };
        for (value, label) in values {
            let normalized = label.to_lowercase();
            if !normalized.starts_with(&reserved)
                || normalized.len() <= reserved.len()
                || label.chars().any(char::is_control)
            {
                return Err(format!(
                    "label {label:?} must be inside the managed namespace"
                ));
            }
            let meaning = format!("{}={value}", field.name);
            if let Some(previous) = seen.insert(normalized, meaning.clone()) {
                return Err(format!(
                    "label {label:?} represents both {previous} and {meaning}"
                ));
            }
        }
    }
    Ok(())
}

fn relations(
    flow: &Flow,
    overrides: Vec<RelationMapping>,
) -> Result<Vec<RelationMapping>, String> {
    let mut relations: Vec<RelationMapping> = flow
        .relations
        .iter()
        .map(|relation| RelationMapping {
            name: relation.name.clone(),
            source: match relation.name.as_str() {
                "parent" => RelationSource::Parent,
                "blocked_by" => RelationSource::Dependency,
                _ => RelationSource::Metadata,
            },
        })
        .collect();
    let mut seen = BTreeSet::new();
    for relation in overrides {
        if !seen.insert(relation.name.clone()) {
            return Err(format!(
                "relation {:?} is mapped more than once",
                relation.name
            ));
        }
        let mapped = relations
            .iter_mut()
            .find(|mapped| mapped.name == relation.name)
            .ok_or_else(|| {
                format!("no relation is called {:?}", relation.name)
            })?;
        *mapped = relation;
    }
    let mut sources = Vec::new();
    for relation in &relations {
        if relation.source != RelationSource::Metadata {
            if sources.contains(&relation.source) {
                return Err(format!(
                    "native relation source {:?} is mapped more than once",
                    relation.source
                ));
            }
            sources.push(relation.source);
        }
    }
    Ok(relations)
}
