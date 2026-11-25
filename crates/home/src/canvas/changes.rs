use alloc::borrow::Cow;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use super::{CanvasError, CodeRef};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Changes {
    pub groups: Vec<ChangeGroup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangeGroup {
    pub kind: ChangeKind,
    pub entries: Vec<Change>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(into = "&'static str", try_from = "String")]
pub enum ChangeKind {
    Core,
    Wiring,
    Mechanical,
}

impl Worded for ChangeKind {
    const ALL: &'static [Self] = &[Self::Core, Self::Wiring, Self::Mechanical];

    fn word(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Wiring => "wiring",
            Self::Mechanical => "mechanical",
        }
    }
}

impl ChangeKind {
    const fn heading(self) -> &'static str {
        match self {
            Self::Core => "Core",
            Self::Wiring => "Wiring",
            Self::Mechanical => "Mechanical",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub code: CodeRef,
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<Tag>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "&'static str", try_from = "String")]
pub enum Tag {
    Subtle,
    Breaking,
    Race,
    Perf,
}

impl Worded for Tag {
    const ALL: &'static [Self] =
        &[Self::Subtle, Self::Breaking, Self::Race, Self::Perf];

    fn word(self) -> &'static str {
        match self {
            Self::Subtle => "subtle",
            Self::Breaking => "breaking",
            Self::Race => "race",
            Self::Perf => "perf",
        }
    }
}

trait Worded: Copy + 'static {
    const ALL: &'static [Self];

    fn word(self) -> &'static str;

    fn parsed(text: &str) -> Result<Self, String> {
        Self::ALL
            .iter()
            .copied()
            .find(|known| known.word() == text)
            .ok_or_else(|| {
                format!(
                    "unknown variant `{text}`, expected one of {}",
                    Self::ALL
                        .iter()
                        .map(|known| format!("`{}`", known.word()))
                        .collect::<Vec<String>>()
                        .join(", ")
                )
            })
    }

    fn schema() -> Schema {
        let words: Vec<&str> =
            Self::ALL.iter().map(|known| known.word()).collect();
        json_schema!({ "type": "string", "enum": words })
    }
}

macro_rules! worded {
    ($kind:ident) => {
        impl From<$kind> for &'static str {
            fn from(kind: $kind) -> Self {
                kind.word()
            }
        }

        impl TryFrom<String> for $kind {
            type Error = String;

            fn try_from(text: String) -> Result<Self, String> {
                Self::parsed(&text)
            }
        }

        impl JsonSchema for $kind {
            fn schema_name() -> Cow<'static, str> {
                Cow::Borrowed(stringify!($kind))
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                Self::schema()
            }
        }
    };
}

worded!(ChangeKind);
worded!(Tag);

impl Changes {
    pub(super) fn check(&self) -> Result<(), CanvasError> {
        self.groups
            .windows(2)
            .find_map(|pair| match pair {
                [before, after] if after.kind == before.kind => {
                    Some(ChangesError::Twice { kind: after.kind })
                }
                [before, after] if after.kind < before.kind => {
                    Some(ChangesError::OutOfOrder {
                        kind: after.kind,
                        after: before.kind,
                    })
                }
                _ => None,
            })
            .map_or(Ok(()), |error| Err(CanvasError::Changes(error)))
    }

    pub(super) fn markdown(&self) -> String {
        self.groups
            .iter()
            .map(|group| {
                let entries: Vec<String> = group
                    .entries
                    .iter()
                    .map(|change| {
                        let tag = change.tag.map_or_else(String::new, |tag| {
                            format!(" ({})", tag.word())
                        });
                        format!(
                            "- {}: {}{tag}\n",
                            change.code.link(),
                            change.note
                        )
                    })
                    .collect();
                format!("**{}**\n\n{}", group.kind.heading(), entries.concat())
            })
            .collect::<Vec<String>>()
            .join("\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangesError {
    OutOfOrder { kind: ChangeKind, after: ChangeKind },
    Twice { kind: ChangeKind },
}

impl fmt::Display for ChangesError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfOrder { kind, after } => write!(
                out,
                "the `{}` group comes after `{}`: order the groups core, wiring, mechanical",
                kind.word(),
                after.word()
            ),
            Self::Twice { kind } => write!(
                out,
                "the `{}` group comes twice: merge them into one",
                kind.word()
            ),
        }
    }
}

impl core::error::Error for ChangesError {}
