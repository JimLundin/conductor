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
use alloc::string::ToString;
use core::fmt::Display;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use toml::Value;

use crate::prelude::*;

pub mod check;
pub mod command;
pub mod eval;
pub mod file;
pub mod graph;
pub mod role;
pub mod snapshot;
mod vocabulary;

pub use vocabulary::{
    Field, FieldRole, FieldType, Flow, Kind, Literal, Predicate, Quantifier,
    Relation, Role, Rule, Severity, Shape, Stage, Status,
};

use crate::issue::Datum;

/// Where the flow is, from the home's root.
pub const PATH: &str = "docs/agents/flow.toml";

/// The flow `text` declares.
///
/// Each table is read by serde in the shape the file writes it; the
/// predicates, whose meaning depends on what the flow declares, are read
/// here.
///
/// # Errors
///
/// The first thing it gets wrong, at its path in the file: a key, a value,
/// a default not of its field's type, a duplicate name, a reference to
/// something it doesn't declare, a role no status has, a field role on two
/// fields or on a field of the wrong type, or a move weighed outside a
/// rule's `when`.
pub fn load(text: &str) -> Result<Flow, String> {
    let top = At::top();
    let file: File = toml::from_str::<Value>(text)
        .map_err(|err| format!("flow: {err}"))
        .and_then(|value| decode_at_path(&top, value))?;
    let mut flow = Flow {
        default_kind: file.default_kind,
        statuses: listed(&top, "status", file.status, |_, read: Status| {
            Ok(read)
        })?,
        fields: listed(&top, "field", file.field, field)?,
        kinds: listed(&top, "kind", file.kind, kind)?,
        relations: listed(
            &top,
            "relation",
            file.relation,
            |_, read: Relation| Ok(read),
        )?,
        stages: listed(&top, "stage", file.stage, |_, read: Stage| Ok(read))?,
        rules: Vec::new(),
    };
    declarations(&flow)?;
    flow.rules =
        listed(&top, "rule", file.rule, |at, entry| rule(&flow, at, entry))?;
    Ok(flow)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    default_kind: String,
    #[serde(default)]
    status: Vec<Value>,
    #[serde(default)]
    field: Vec<Value>,
    #[serde(default)]
    kind: Vec<Value>,
    #[serde(default)]
    relation: Vec<Value>,
    #[serde(default)]
    stage: Vec<Value>,
    #[serde(default)]
    rule: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FieldEntry {
    name: String,
    #[serde(rename = "type")]
    of: TypeWord,
    #[serde(default)]
    values: Vec<String>,
    default: Option<Datum>,
    role: Option<FieldRole>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum TypeWord {
    Enum,
    Text,
    Texts,
    Time,
    Bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KindEntry {
    name: String,
    label: Option<String>,
    icon: Option<String>,
    colour: Option<String>,
    #[serde(default)]
    sections: Vec<String>,
    #[serde(default)]
    fields: Vec<Value>,
    #[serde(default)]
    order: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleEntry {
    message: String,
    severity: Severity,
    when: Value,
    require: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AcrossEntry {
    via: String,
    #[serde(rename = "where")]
    condition: Value,
}

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

fn decode_at_path<T: DeserializeOwned>(
    at: &At,
    value: Value,
) -> Result<T, String> {
    T::deserialize(value)
        .or_else(|err| at.fail(err.to_string().trim_end().replace('\n', ", ")))
}

fn listed<E: DeserializeOwned, T: Named>(
    at: &At,
    key: &str,
    entries: Vec<Value>,
    mut read: impl FnMut(&At, E) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    let here = at.key(key);
    let mut read_all: Vec<T> = Vec::new();
    for (index, entry) in entries.into_iter().enumerate() {
        let entry_at = here.index(index);
        let item = read(&entry_at, decode_at_path(&entry_at, entry)?)?;
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

fn field(at: &At, entry: FieldEntry) -> Result<Field, String> {
    let value = match entry.of {
        TypeWord::Enum if entry.values.is_empty() => {
            return at.key("values").fail("an enum needs at least one value");
        }
        TypeWord::Enum => FieldType::Enum(entry.values),
        TypeWord::Text => FieldType::Text,
        TypeWord::Texts => FieldType::Texts,
        TypeWord::Time => FieldType::Time,
        TypeWord::Bool => FieldType::Bool,
    };
    if let Some(Err(problem)) =
        entry.default.as_ref().map(|given| admitted(&value, given))
    {
        return at.key("default").fail(problem);
    }
    if let Some(held) = entry.role
        && value != held.takes()
    {
        return at.key("role").fail(format!(
            "a field with the role {:?} has type {}",
            held.word(),
            type_word(&held.takes())
        ));
    }
    Ok(Field {
        name: entry.name,
        value,
        default: entry.default,
        role: entry.role,
    })
}

/// Whether `datum` is a value of type `value`, and what it is not when it
/// isn't.
///
/// # Errors
///
/// What `datum` is not: one of an enum's values, text, a time, a list of
/// text, or true or false.
pub fn admitted(value: &FieldType, datum: &Datum) -> Result<(), String> {
    match (value, datum) {
        (FieldType::Enum(words), Datum::Text(word))
            if !words.contains(word) =>
        {
            Err(format!("{word:?} is not one of {}", words.join(", ")))
        }
        (
            FieldType::Enum(_) | FieldType::Text | FieldType::Time,
            Datum::Text(_),
        )
        | (FieldType::Texts, Datum::Texts(_))
        | (FieldType::Bool, Datum::Bool(_)) => Ok(()),
        (FieldType::Enum(words), _) => {
            Err(format!("is not one of {}", words.join(", ")))
        }
        (FieldType::Text, _) => Err(String::from("is not text")),
        (FieldType::Time, _) => Err(String::from("is not a time")),
        (FieldType::Texts, _) => Err(String::from("is not a list of text")),
        (FieldType::Bool, _) => Err(String::from("is not true or false")),
    }
}

const fn type_word(value: &FieldType) -> &'static str {
    match value {
        FieldType::Enum(_) => "enum",
        FieldType::Text => "text",
        FieldType::Texts => "texts",
        FieldType::Time => "time",
        FieldType::Bool => "bool",
    }
}

fn kind(at: &At, entry: KindEntry) -> Result<Kind, String> {
    Ok(Kind {
        label: entry.label.unwrap_or_else(|| entry.name.clone()),
        name: entry.name,
        icon: entry.icon,
        colour: entry.colour,
        sections: entry.sections,
        fields: listed(at, "fields", entry.fields, field)?,
        order: entry.order,
    })
}

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
    let mut roles = Vec::new();
    for (index, field) in flow.fields.iter().enumerate() {
        role_once(&mut roles, top.key("field").index(index), field)?;
    }
    for (index, kind) in flow.kinds.iter().enumerate() {
        let at = top.key("kind").index(index);
        for (place, field) in kind.fields.iter().enumerate() {
            role_once(&mut roles, at.key("fields").index(place), field)?;
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

fn role_once(
    seen: &mut Vec<(FieldRole, At)>,
    at: At,
    field: &Field,
) -> Result<(), String> {
    let Some(role) = field.role else {
        return Ok(());
    };
    if let Some((_, first)) = seen.iter().find(|(other, _)| *other == role) {
        return at.key("role").fail(format!(
            "{} also has the role {:?}",
            first.0,
            role.word()
        ));
    }
    seen.push((role, at));
    Ok(())
}

fn shown(flow: &Flow, kind: &Kind, name: &str) -> bool {
    matches!(name, "status" | "stage")
        || flow.field(&kind.name, name).is_some()
        || flow.relations.iter().any(|relation| {
            relation.name == name
                && (relation.from.is_empty()
                    || relation.from.contains(&kind.name))
        })
}

fn rule(flow: &Flow, at: &At, entry: RuleEntry) -> Result<Rule, String> {
    let when = PredicateReader {
        flow,
        allows_move_predicates: true,
    }
    .read(&at.key("when"), &entry.when)?;
    let require = PredicateReader {
        flow,
        allows_move_predicates: weighs_a_move(&when),
    }
    .read(&at.key("require"), &entry.require)?;
    Ok(Rule {
        message: entry.message,
        severity: entry.severity,
        when,
        require,
    })
}

fn weighs_a_move(predicate: &Predicate) -> bool {
    match predicate {
        Predicate::From(_) | Predicate::To(_) => true,
        Predicate::Not(inner) => weighs_a_move(inner),
        Predicate::Across { condition, .. } => weighs_a_move(condition),
        Predicate::All(parts) | Predicate::Any(parts) => {
            parts.iter().any(weighs_a_move)
        }
        _ => false,
    }
}

#[derive(Clone, Copy)]
struct PredicateReader<'a> {
    flow: &'a Flow,
    allows_move_predicates: bool,
}

impl PredicateReader<'_> {
    fn read(self, at: &At, value: &Value) -> Result<Predicate, String> {
        let Value::Table(table) = value else {
            return at.fail("is not a predicate, an inline table of one key");
        };
        let mut entries = table.iter();
        let (Some((key, inner)), None) = (entries.next(), entries.next())
        else {
            return at.fail("a predicate has exactly one key");
        };
        let here = at.key(key);
        let flow = self.flow;
        let nested = |inner: &Value| self.read(&here, inner).map(Box::new);
        match key.as_str() {
            "kind" => {
                declared(&here, inner, "kind", |name| flow.kind(name).is_some())
                    .map(Predicate::Kind)
            }
            "stage" => declared(&here, inner, "stage", |name| {
                flow.stage(name).is_some()
            })
            .map(Predicate::Stage),
            "status_role" => Role::deserialize(inner.clone())
                .map(Predicate::StatusRole)
                .or_else(|err| here.fail(err.message())),
            "has" => declared(&here, inner, "field or relation", |name| {
                name == "stage"
                    || some_field(flow, name).is_some()
                    || flow
                        .relations
                        .iter()
                        .any(|relation| relation.name == name)
            })
            .map(Predicate::Has),
            "has_section" => word_of(&here, inner).map(Predicate::HasSection),
            "field" => field_atom(flow, &here, inner),
            "from" | "to" if !self.allows_move_predicates => here.fail(
                "a rule's require weighs a move only where its when does",
            ),
            "from" => nested(inner).map(Predicate::From),
            "to" => nested(inner).map(Predicate::To),
            "not" => nested(inner).map(Predicate::Not),
            "all" | "any" | "none" => self.combined(&here, key, inner),
            _ => here.fail(
                "is not an atom (kind, stage, status_role, field, has, \
                 has_section, from, to), a connective (all, any, not) or a \
                 quantifier (any, all, none)",
            ),
        }
    }

    fn combined(
        self,
        at: &At,
        key: &str,
        value: &Value,
    ) -> Result<Predicate, String> {
        match value {
            Value::Array(items) if key != "none" => {
                let each = items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| self.read(&at.index(index), item))
                    .collect::<Result<Vec<Predicate>, String>>()?;
                Ok(if key == "all" {
                    Predicate::All(each)
                } else {
                    Predicate::Any(each)
                })
            }
            Value::Table(_) => {
                let across: AcrossEntry = decode_at_path(at, value.clone())?;
                if self.flow.relation(&across.via).is_none() {
                    return at.key("via").fail(format!(
                        "no relation or reverse is called {:?}",
                        across.via
                    ));
                }
                let condition =
                    self.read(&at.key("where"), &across.condition)?;
                let quantifier = match key {
                    "all" => Quantifier::All,
                    "any" => Quantifier::Any,
                    _ => Quantifier::None,
                };
                Ok(Predicate::Across {
                    quantifier,
                    via: across.via,
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
    let Some(field) = some_field(flow, name) else {
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
        (FieldType::Time, Value::Datetime(at)) => Literal::Text(at.to_string()),
        _ => return here.fail("is not text"),
    };
    Ok(Predicate::Field {
        name: name.clone(),
        value,
    })
}

fn some_field<'flow>(flow: &'flow Flow, name: &str) -> Option<&'flow Field> {
    flow.kinds
        .iter()
        .find_map(|kind| flow.field(&kind.name, name))
}

#[cfg(test)]
mod tests;
