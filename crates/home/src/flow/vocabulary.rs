use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::issue::Datum;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Flow {
    pub default_kind: String,
    pub statuses: Vec<Status>,
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
    Idle,
    Startable,
    Active,
    Review,
    Terminal,
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
    pub default: Option<Datum>,
    pub role: Option<FieldRole>,
}

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
    Labels,
    Branch,
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
    pub sections: Vec<String>,
    pub fields: Vec<Field>,
    pub order: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Relation {
    pub name: String,
    #[serde(default)]
    pub from: Vec<String>,
    #[serde(default)]
    pub to: Vec<String>,
    #[serde(default)]
    pub many: bool,
    #[serde(default)]
    pub cycles: bool,
    pub shape: Shape,
    pub reverse: String,
}

impl Relation {
    #[must_use]
    pub fn starts_from(&self, kind: &str) -> bool {
        any_or_listed(&self.from, kind)
    }

    #[must_use]
    pub fn reaches(&self, kind: &str) -> bool {
        any_or_listed(&self.to, kind)
    }
}

fn any_or_listed(kinds: &[String], kind: &str) -> bool {
    kinds.is_empty() || kinds.iter().any(|listed| listed == kind)
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    Hierarchy,
    Dependency,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    pub name: String,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub next: Vec<String>,
    #[serde(default)]
    pub sign_off: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Rule {
    pub message: String,
    pub severity: Severity,
    pub when: Predicate,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Predicate {
    Kind(String),
    Stage(String),
    StatusRole(Role),
    Field {
        name: String,
        value: Literal,
    },
    Has(String),
    HasSection(String),
    From(Box<Self>),
    To(Box<Self>),
    All(Vec<Self>),
    Any(Vec<Self>),
    Not(Box<Self>),
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

    pub fn fields_of(&self, kind: &str) -> impl Iterator<Item = &Field> {
        let own = self.kind(kind).map_or(&[][..], |declared| &declared.fields);
        self.fields.iter().chain(own)
    }

    #[must_use]
    pub fn field(&self, kind: &str, name: &str) -> Option<&Field> {
        self.fields_of(kind).find(|field| field.name == name)
    }

    #[must_use]
    pub fn field_with(&self, kind: &str, role: FieldRole) -> Option<&Field> {
        self.fields_of(kind).find(|field| field.role == Some(role))
    }
}
