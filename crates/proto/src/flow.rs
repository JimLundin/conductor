//! The flow's vocabulary, as the home's `docs/agents/flow.toml` declares it.
//!
//! The kinds of issue and their fields, how issues relate, their statuses
//! and stages, and the rules they should meet (the home's ADR 0001). This
//! is the one definition, as data; the home crate loads, reads and
//! evaluates it.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Datum;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Flow {
    /// The kind of an issue that names none.
    pub default_kind: String,
    pub statuses: Vec<Status>,
    /// The fields every kind has.
    pub fields: Vec<Field>,
    pub kinds: Vec<Kind>,
    pub relations: Vec<Relation>,
    pub stages: Vec<Stage>,
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub name: String,
    pub role: Role,
    pub colour: Option<String>,
}

/// What a status means to Conductor, whatever the flow calls it.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Waiting to be taken up.
    Idle,
    /// Ready for a session to start on.
    Startable,
    /// Being worked on.
    Active,
    /// Waiting for someone to look at what was done.
    Review,
    /// Finished.
    Terminal,
    /// Given up.
    Dropped,
}

impl Role {
    pub const ALL: [Self; 6] = [
        Self::Idle,
        Self::Startable,
        Self::Active,
        Self::Review,
        Self::Terminal,
        Self::Dropped,
    ];

    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Startable => "startable",
            Self::Active => "active",
            Self::Review => "review",
            Self::Terminal => "terminal",
            Self::Dropped => "dropped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "FlowField")]
pub struct Field {
    pub name: String,
    pub value: FieldType,
    /// What an issue that leaves it out has, of its type.
    pub default: Option<Datum>,
    pub role: Option<FieldRole>,
}

/// What a field means to Conductor, whatever the flow calls it. At most
/// one field has each.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum FieldRole {
    /// The words the app shows on an issue and edits beside its title.
    Labels,
    /// The branch its sessions land on, in place of `issue/<id>`.
    Branch,
    /// When it was opened, which opening it writes.
    Created,
}

impl FieldRole {
    pub const ALL: [Self; 3] = [Self::Labels, Self::Branch, Self::Created];

    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Labels => "labels",
            Self::Branch => "branch",
            Self::Created => "created",
        }
    }

    /// The type its field has.
    #[must_use]
    pub const fn takes(self) -> FieldType {
        match self {
            Self::Labels => FieldType::Texts,
            Self::Branch => FieldType::Text,
            Self::Created => FieldType::Time,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum FieldType {
    /// One of these words.
    Enum(Vec<String>),
    Text,
    Texts,
    Time,
    Bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Kind {
    pub name: String,
    pub label: String,
    pub icon: Option<String>,
    pub colour: Option<String>,
    /// The headings its body is expected to have.
    pub sections: Vec<String>,
    /// Its own fields, beyond those every kind has.
    pub fields: Vec<Field>,
    /// The fields and relations the app shows first, in this order.
    pub order: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Relation {
    pub name: String,
    /// The kinds it may start from; any kind when empty.
    #[serde(default)]
    pub from: Vec<String>,
    /// The kinds it may point to; any kind when empty.
    #[serde(default)]
    pub to: Vec<String>,
    #[serde(default)]
    pub many: bool,
    #[serde(default)]
    pub cycles: bool,
    pub shape: Shape,
    /// Its name followed backwards, from the issue it points to.
    pub reverse: String,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    /// Drawn as a tree.
    Hierarchy,
    /// Drawn as a graph.
    Dependency,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    pub name: String,
    /// The skills that serve it, the first one leading.
    #[serde(default)]
    pub skills: Vec<String>,
    /// The stages it usually moves to; any other move is unusual.
    #[serde(default)]
    pub next: Vec<String>,
    /// Whether the owner signs off before work moves on.
    #[serde(default)]
    pub sign_off: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Rule {
    pub message: String,
    pub severity: Severity,
    /// The issues it applies to.
    pub when: Predicate,
    /// What those issues should meet.
    pub require: Predicate,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warn,
    Error,
}

/// A condition on an issue, among the issues it relates to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Predicate {
    Kind(String),
    Stage(String),
    StatusRole(Role),
    Field {
        name: String,
        value: Literal,
    },
    /// It has this field, this relation, or a stage.
    Has(String),
    HasSection(String),
    /// While a move is weighed: the issue as it was. Only a rule's
    /// `when` has it.
    From(Box<Self>),
    /// While a move is weighed: the issue as it would be. Only a rule's
    /// `when` has it.
    To(Box<Self>),
    All(Vec<Self>),
    Any(Vec<Self>),
    Not(Box<Self>),
    /// The issues reached through a relation, or its reverse, meet the
    /// condition: any, all or none of them.
    Across {
        quantifier: Quantifier,
        via: String,
        condition: Box<Self>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Literal {
    Text(String),
    Bool(bool),
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum Quantifier {
    Any,
    All,
    None,
}

impl Flow {
    #[must_use]
    pub fn kind(&self, name: &str) -> Option<&Kind> {
        self.kinds.iter().find(|kind| kind.name == name)
    }

    #[must_use]
    pub fn stage(&self, name: &str) -> Option<&Stage> {
        self.stages.iter().find(|stage| stage.name == name)
    }

    #[must_use]
    pub fn status(&self, name: &str) -> Option<&Status> {
        self.statuses.iter().find(|status| status.name == name)
    }

    /// The relation called `name`, and whether `name` is its reverse.
    #[must_use]
    pub fn relation(&self, name: &str) -> Option<(&Relation, bool)> {
        self.relations.iter().find_map(|relation| {
            if relation.name == name {
                Some((relation, false))
            } else {
                (relation.reverse == name).then_some((relation, true))
            }
        })
    }

    /// The fields an issue of `kind` has: those every kind has, then the
    /// kind's own.
    pub fn fields_of(&self, kind: &str) -> impl Iterator<Item = &Field> {
        let own = self.kind(kind).map_or(&[][..], |declared| &declared.fields);
        self.fields.iter().chain(own)
    }

    /// The field called `name` of an issue of `kind`.
    #[must_use]
    pub fn field(&self, kind: &str, name: &str) -> Option<&Field> {
        self.fields_of(kind).find(|field| field.name == name)
    }

    /// The field of an issue of `kind` that has `role`.
    #[must_use]
    pub fn field_with(&self, kind: &str, role: FieldRole) -> Option<&Field> {
        self.fields_of(kind).find(|field| field.role == Some(role))
    }
}
