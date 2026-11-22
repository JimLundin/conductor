//! The flow: the home's `docs/agents/flow.toml` (its ADR 0001).
//!
//! It declares the kinds of issue and their fields, how issues relate,
//! their statuses and stages, and the rules they should meet. Nothing here
//! names a kind, status or stage: this models the vocabulary the flow is
//! written in, and the flow is data the owner changes without changing
//! Conductor.
//!
//! Lists are arrays of tables, so the flow's order is the order it
//! declares, and the app shows stages, statuses and kinds in that order.

use alloc::boxed::Box;
use core::fmt::Display;

use toml::{Table, Value};

use crate::prelude::*;

pub mod check;
pub mod command;
pub mod eval;
pub mod graph;

pub use proto::flow::{
    Field, FieldType, Flow, Kind, Literal, Predicate, Quantifier, Relation,
    Role, Rule, Severity, Shape, Stage, Status,
};

/// The flow `text` declares.
///
/// # Errors
///
/// The first thing it gets wrong, at its path in the file: a key, a value,
/// a duplicate name, a reference to something it doesn't declare, or a
/// role no status has.
pub fn load(text: &str) -> Result<Flow, String> {
    let table =
        toml::from_str::<Table>(text).map_err(|err| format!("flow: {err}"))?;
    let top = At::top();
    known(
        &top,
        &table,
        &[
            "default_kind",
            "status",
            "field",
            "kind",
            "relation",
            "stage",
            "rule",
        ],
    )?;
    let statuses = listed(&top, &table, "status", status)?;
    let fields = listed(&top, &table, "field", field)?;
    let kinds = listed(&top, &table, "kind", kind)?;
    let relations = listed(&top, &table, "relation", relation)?;
    let stages = listed(&top, &table, "stage", stage)?;
    let default_kind = text_of(&top, &table, "default_kind")?;
    let mut flow = Flow {
        default_kind,
        statuses,
        fields,
        kinds,
        relations,
        stages,
        rules: Vec::new(),
    };
    declarations(&flow)?;
    flow.rules =
        listed(&top, &table, "rule", |at, entry| rule(&flow, at, entry))?;
    Ok(flow)
}

/// Where a key or value is in the file, as `rule[0].require.any`.
#[derive(Debug, Clone)]
struct At(String);

impl At {
    const fn top() -> Self {
        Self(String::new())
    }

    fn key(&self, key: &str) -> Self {
        if self.0.is_empty() {
            Self(key.to_owned())
        } else {
            Self(format!("{}.{key}", self.0))
        }
    }

    fn index(&self, index: usize) -> Self {
        Self(format!("{}[{index}]", self.0))
    }

    fn fail<T>(&self, problem: impl Display) -> Result<T, String> {
        if self.0.is_empty() {
            Err(format!("flow: {problem}"))
        } else {
            Err(format!("{}: {problem}", self.0))
        }
    }
}

fn known(at: &At, table: &Table, keys: &[&str]) -> Result<(), String> {
    table
        .keys()
        .find(|key| !keys.contains(&key.as_str()))
        .map_or(Ok(()), |key| {
            at.key(key).fail(format!(
                "is not a key here; the keys are {}",
                keys.join(", ")
            ))
        })
}

/// The array of tables under `key`, each read by `read`, with no two
/// sharing a name.
fn listed<T: Named>(
    at: &At,
    table: &Table,
    key: &str,
    mut read: impl FnMut(&At, &Table) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    let here = at.key(key);
    let entries = match table.get(key) {
        None => return Ok(Vec::new()),
        Some(Value::Array(entries)) => entries,
        Some(_) => return here.fail("is not a list of tables"),
    };
    let mut read_all: Vec<T> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let entry_at = here.index(index);
        let Value::Table(fields) = entry else {
            return entry_at.fail("is not a table");
        };
        let item = read(&entry_at, fields)?;
        if let Some(name) = item.name()
            && read_all.iter().any(|other| other.name() == Some(name))
        {
            return entry_at
                .key("name")
                .fail(format!("another {key} is also called {name:?}"));
        }
        read_all.push(item);
    }
    Ok(read_all)
}

trait Named {
    fn name(&self) -> Option<&str>;
}

macro_rules! named {
    ($($item:ty),*) => {
        $(impl Named for $item {
            fn name(&self) -> Option<&str> {
                Some(&self.name)
            }
        })*
    };
}

named!(Status, Field, Kind, Relation, Stage);

impl Named for Rule {
    fn name(&self) -> Option<&str> {
        None
    }
}

fn text_of(at: &At, table: &Table, key: &str) -> Result<String, String> {
    optional_text(at, table, key)?
        .map_or_else(|| at.key(key).fail("is missing"), Ok)
}

fn optional_text(
    at: &At,
    table: &Table,
    key: &str,
) -> Result<Option<String>, String> {
    match table.get(key) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => at.key(key).fail("is not text"),
    }
}

fn texts(at: &At, table: &Table, key: &str) -> Result<Vec<String>, String> {
    let here = at.key(key);
    match table.get(key) {
        None => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .iter()
            .enumerate()
            .map(|(index, value)| match value {
                Value::String(text) => Ok(text.clone()),
                _ => here.index(index).fail("is not text"),
            })
            .collect(),
        Some(_) => here.fail("is not a list of text"),
    }
}

fn flag(at: &At, table: &Table, key: &str) -> Result<bool, String> {
    match table.get(key) {
        None => Ok(false),
        Some(Value::Boolean(value)) => Ok(*value),
        Some(_) => at.key(key).fail("is not true or false"),
    }
}

fn status(at: &At, table: &Table) -> Result<Status, String> {
    known(at, table, &["name", "role", "colour"])?;
    let word = text_of(at, table, "role")?;
    let role = Role::named(&word).map_or_else(
        || {
            at.key("role").fail(format!(
                "{word:?} is not a role; the roles are {}",
                Role::ALL.map(Role::word).join(", ")
            ))
        },
        Ok,
    )?;
    Ok(Status {
        name: text_of(at, table, "name")?,
        role,
        colour: optional_text(at, table, "colour")?,
    })
}

fn field(at: &At, table: &Table) -> Result<Field, String> {
    known(at, table, &["name", "type", "values", "to", "default"])?;
    let word = text_of(at, table, "type")?;
    let value = match word.as_str() {
        "enum" => FieldType::Enum(texts(at, table, "values")?),
        "ref" => FieldType::Ref(texts(at, table, "to")?),
        "refs" => FieldType::Refs(texts(at, table, "to")?),
        "text" => FieldType::Text,
        "texts" => FieldType::Texts,
        "time" => FieldType::Time,
        "bool" => FieldType::Bool,
        _ => {
            return at.key("type").fail(format!(
                "{word:?} is not a field type; the types are enum, ref, \
                 refs, text, texts, time, bool"
            ));
        }
    };
    let name = text_of(at, table, "name")?;
    let default = optional_text(at, table, "default")?;
    if let FieldType::Enum(values) = &value {
        if values.is_empty() {
            return at.key("values").fail("an enum needs at least one value");
        }
        if let Some(missing) =
            default.as_ref().filter(|word| !values.contains(word))
        {
            return at
                .key("default")
                .fail(format!("{missing:?} is not one of its values"));
        }
    }
    Ok(Field {
        name,
        value,
        default,
    })
}

fn kind(at: &At, table: &Table) -> Result<Kind, String> {
    known(
        at,
        table,
        &[
            "name", "label", "icon", "colour", "sections", "fields", "order",
        ],
    )?;
    let name = text_of(at, table, "name")?;
    Ok(Kind {
        label: optional_text(at, table, "label")?
            .unwrap_or_else(|| name.clone()),
        name,
        icon: optional_text(at, table, "icon")?,
        colour: optional_text(at, table, "colour")?,
        sections: texts(at, table, "sections")?,
        fields: listed(at, table, "fields", field)?,
        order: texts(at, table, "order")?,
    })
}

fn relation(at: &At, table: &Table) -> Result<Relation, String> {
    known(
        at,
        table,
        &["name", "from", "to", "many", "cycles", "shape", "reverse"],
    )?;
    let word = text_of(at, table, "shape")?;
    let shape = match word.as_str() {
        "hierarchy" => Shape::Hierarchy,
        "dependency" => Shape::Dependency,
        _ => {
            return at.key("shape").fail(format!(
                "{word:?} is not a shape; the shapes are hierarchy, dependency"
            ));
        }
    };
    Ok(Relation {
        name: text_of(at, table, "name")?,
        from: texts(at, table, "from")?,
        to: texts(at, table, "to")?,
        many: flag(at, table, "many")?,
        cycles: flag(at, table, "cycles")?,
        shape,
        reverse: text_of(at, table, "reverse")?,
    })
}

fn stage(at: &At, table: &Table) -> Result<Stage, String> {
    known(at, table, &["name", "skills", "next", "sign_off"])?;
    Ok(Stage {
        name: text_of(at, table, "name")?,
        skills: texts(at, table, "skills")?,
        next: texts(at, table, "next")?,
        sign_off: flag(at, table, "sign_off")?,
    })
}

/// The names the flow's parts use declare something it has.
fn declarations(flow: &Flow) -> Result<(), String> {
    let top = At::top();
    if let Some(role) = Role::ALL
        .into_iter()
        .find(|role| flow.statuses.iter().all(|status| status.role != *role))
    {
        return top.fail(format!("no status has the role {:?}", role.word()));
    }
    if flow.kind(&flow.default_kind).is_none() {
        return top
            .key("default_kind")
            .fail(format!("no kind is called {:?}", flow.default_kind));
    }
    for (index, relation) in flow.relations.iter().enumerate() {
        let at = top.key("relation").index(index);
        kinds_exist(flow, &at.key("from"), &relation.from)?;
        kinds_exist(flow, &at.key("to"), &relation.to)?;
        if flow.relations.iter().enumerate().any(|(place, other)| {
            other.name == relation.reverse
                || other.reverse == relation.name
                || (place != index && other.reverse == relation.reverse)
        }) {
            return at.key("reverse").fail(format!(
                "{:?} is already a relation's name or reverse",
                relation.reverse
            ));
        }
    }
    for (index, field) in flow.fields.iter().enumerate() {
        field_refers(flow, &top.key("field").index(index), field)?;
    }
    for (index, kind) in flow.kinds.iter().enumerate() {
        let at = top.key("kind").index(index);
        for (place, field) in kind.fields.iter().enumerate() {
            field_refers(flow, &at.key("fields").index(place), field)?;
        }
        for (place, name) in kind.order.iter().enumerate() {
            if !shown(flow, kind, name) {
                return at.key("order").index(place).fail(format!(
                    "{name:?} is not a field or relation of {:?}",
                    kind.name
                ));
            }
        }
    }
    for (index, stage) in flow.stages.iter().enumerate() {
        let at = top.key("stage").index(index).key("next");
        for (place, next) in stage.next.iter().enumerate() {
            if flow.stage(next).is_none() {
                return at
                    .index(place)
                    .fail(format!("no stage is called {next:?}"));
            }
        }
    }
    Ok(())
}

fn kinds_exist(flow: &Flow, at: &At, kinds: &[String]) -> Result<(), String> {
    kinds
        .iter()
        .enumerate()
        .find(|(_, name)| flow.kind(name).is_none())
        .map_or(Ok(()), |(index, name)| {
            at.index(index).fail(format!("no kind is called {name:?}"))
        })
}

fn field_refers(flow: &Flow, at: &At, field: &Field) -> Result<(), String> {
    match &field.value {
        FieldType::Ref(kinds) | FieldType::Refs(kinds) => {
            kinds_exist(flow, &at.key("to"), kinds)
        }
        _ => Ok(()),
    }
}

/// Whether the app can show `name` on an issue of `kind`.
fn shown(flow: &Flow, kind: &Kind, name: &str) -> bool {
    matches!(name, "status" | "stage")
        || flow.fields.iter().any(|field| field.name == name)
        || kind.fields.iter().any(|field| field.name == name)
        || flow.relations.iter().any(|relation| {
            relation.name == name
                && (relation.from.is_empty()
                    || relation.from.contains(&kind.name))
        })
}

fn rule(flow: &Flow, at: &At, table: &Table) -> Result<Rule, String> {
    known(at, table, &["message", "severity", "when", "require"])?;
    let word = text_of(at, table, "severity")?;
    let severity = match word.as_str() {
        "warn" => Severity::Warn,
        "error" => Severity::Error,
        _ => {
            return at.key("severity").fail(format!(
                "{word:?} is not a severity; the severities are warn, error"
            ));
        }
    };
    let condition = |key: &str| {
        table.get(key).map_or_else(
            || at.key(key).fail("is missing"),
            |value| predicate(flow, &at.key(key), value),
        )
    };
    Ok(Rule {
        message: text_of(at, table, "message")?,
        severity,
        when: condition("when")?,
        require: condition("require")?,
    })
}

/// The predicate `value` writes: an inline table of one key.
fn predicate(flow: &Flow, at: &At, value: &Value) -> Result<Predicate, String> {
    let Value::Table(table) = value else {
        return at.fail("is not a predicate, an inline table of one key");
    };
    let mut entries = table.iter();
    let (Some((key, inner)), None) = (entries.next(), entries.next()) else {
        return at.fail("a predicate has exactly one key");
    };
    let here = at.key(key);
    let nested = |inner: &Value| predicate(flow, &here, inner).map(Box::new);
    match key.as_str() {
        "kind" => {
            declared(&here, inner, "kind", |name| flow.kind(name).is_some())
                .map(Predicate::Kind)
        }
        "stage" => {
            declared(&here, inner, "stage", |name| flow.stage(name).is_some())
                .map(Predicate::Stage)
        }
        "status_role" => role_of(&here, inner).map(Predicate::StatusRole),
        "has" => declared(&here, inner, "field or relation", |name| {
            name == "stage"
                || flow.field(name).is_some()
                || flow.relations.iter().any(|relation| relation.name == name)
        })
        .map(Predicate::Has),
        "has_section" => word_of(&here, inner).map(Predicate::HasSection),
        "field" => field_atom(flow, &here, inner),
        "from" => nested(inner).map(Predicate::From),
        "to" => nested(inner).map(Predicate::To),
        "not" => nested(inner).map(Predicate::Not),
        "all" | "any" | "none" => combined(flow, &here, key, inner),
        _ => here.fail(
            "is not an atom (kind, stage, status_role, field, has, \
             has_section, from, to), a connective (all, any, not) or a \
             quantifier (any, all, none)",
        ),
    }
}

fn word_of(at: &At, value: &Value) -> Result<String, String> {
    match value {
        Value::String(word) => Ok(word.clone()),
        _ => at.fail("is not text"),
    }
}

fn declared(
    at: &At,
    value: &Value,
    what: &str,
    exists: impl Fn(&str) -> bool,
) -> Result<String, String> {
    let name = word_of(at, value)?;
    if exists(&name) {
        Ok(name)
    } else {
        at.fail(format!("no {what} is called {name:?}"))
    }
}

fn role_of(at: &At, value: &Value) -> Result<Role, String> {
    let word = word_of(at, value)?;
    Role::named(&word).map_or_else(
        || {
            at.fail(format!(
                "{word:?} is not a role; the roles are {}",
                Role::ALL.map(Role::word).join(", ")
            ))
        },
        Ok,
    )
}

fn field_atom(
    flow: &Flow,
    at: &At,
    value: &Value,
) -> Result<Predicate, String> {
    let Value::Table(table) = value else {
        return at
            .fail("is not a field and its value, as { <field> = <value> }");
    };
    let mut entries = table.iter();
    let (Some((name, literal)), None) = (entries.next(), entries.next()) else {
        return at.fail("names exactly one field");
    };
    let here = at.key(name);
    let Some(field) = flow.field(name) else {
        return here.fail("no field is called that");
    };
    let value = match (&field.value, literal) {
        (FieldType::Bool, Value::Boolean(flag)) => Literal::Bool(*flag),
        (FieldType::Enum(words), Value::String(word))
            if !words.contains(word) =>
        {
            return here.fail(format!("{word:?} is not one of its values"));
        }
        (FieldType::Bool, _) => return here.fail("is not true or false"),
        (_, Value::String(word)) => Literal::Text(word.clone()),
        _ => return here.fail("is not text"),
    };
    Ok(Predicate::Field {
        name: name.clone(),
        value,
    })
}

/// `all` or `any` of a list, or `any`, `all` or `none` of a relation's
/// issues.
fn combined(
    flow: &Flow,
    at: &At,
    key: &str,
    value: &Value,
) -> Result<Predicate, String> {
    match value {
        Value::Array(items) if key != "none" => {
            let each = items
                .iter()
                .enumerate()
                .map(|(index, item)| predicate(flow, &at.index(index), item))
                .collect::<Result<Vec<Predicate>, String>>()?;
            Ok(if key == "all" {
                Predicate::All(each)
            } else {
                Predicate::Any(each)
            })
        }
        Value::Table(table) => {
            known(at, table, &["via", "where"])?;
            let via = table.get("via").map_or_else(
                || at.key("via").fail("is missing"),
                |given| {
                    declared(
                        &at.key("via"),
                        given,
                        "relation or reverse",
                        |name| flow.relation(name).is_some(),
                    )
                },
            )?;
            let condition = table.get("where").map_or_else(
                || at.key("where").fail("is missing"),
                |inner| predicate(flow, &at.key("where"), inner),
            )?;
            let quantifier = match key {
                "all" => Quantifier::All,
                "any" => Quantifier::Any,
                _ => Quantifier::None,
            };
            Ok(Predicate::Across {
                quantifier,
                via,
                condition: Box::new(condition),
            })
        }
        _ if key == "none" => {
            at.fail("takes { via = <relation>, where = <predicate> }")
        }
        _ => at.fail(
            "takes a list of predicates, or { via = <relation>, where = \
             <predicate> }",
        ),
    }
}

#[cfg(test)]
mod tests;
