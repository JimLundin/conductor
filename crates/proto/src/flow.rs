//! The flow's vocabulary, as the home's `docs/agents/flow.toml` declares it.
//!
//! The kinds of issue and their fields, how issues relate, their statuses
//! and stages, and the rules they should meet (the home's ADR 0001). This
//! is the one definition, as data; the home crate loads, reads and
//! evaluates it.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub name: String,
    pub role: Role,
    pub colour: Option<String>,
}

/// What a status means to Conductor, whatever the flow calls it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
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

    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.word() == word)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub value: FieldType,
    /// What an issue that leaves it out has.
    pub default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldType {
    /// One of these words.
    Enum(Vec<String>),
    /// The id of an issue of one of these kinds, or of any kind when empty.
    Ref(Vec<String>),
    /// Ids of issues, as for [`FieldType::Ref`].
    Refs(Vec<String>),
    Text,
    Texts,
    Time,
    Bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relation {
    pub name: String,
    /// The kinds it may start from; any kind when empty.
    pub from: Vec<String>,
    /// The kinds it may point to; any kind when empty.
    pub to: Vec<String>,
    pub many: bool,
    pub cycles: bool,
    pub shape: Shape,
    /// Its name followed backwards, from the issue it points to.
    pub reverse: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Shape {
    /// Drawn as a tree.
    Hierarchy,
    /// Drawn as a graph.
    Dependency,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stage {
    pub name: String,
    /// The skills that serve it, the first one leading.
    pub skills: Vec<String>,
    /// The stages it usually moves to; any other move is unusual.
    pub next: Vec<String>,
    /// Whether the owner signs off before work moves on.
    pub sign_off: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub message: String,
    pub severity: Severity,
    /// The issues it applies to.
    pub when: Predicate,
    /// What those issues should meet.
    pub require: Predicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Warn,
    Error,
}

/// A condition on an issue, among the issues it relates to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// While a move is weighed: the issue as it was.
    From(Box<Self>),
    /// While a move is weighed: the issue as it would be.
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Literal {
    Text(String),
    Bool(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

    /// The field called `name`: one every kind has, else the first kind's
    /// that has it.
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields
            .iter()
            .chain(self.kinds.iter().flat_map(|kind| &kind.fields))
            .find(|field| field.name == name)
    }
}
