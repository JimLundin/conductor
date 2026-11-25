use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{CanvasError, CodeRef, wire};

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
pub enum ChangeKind {
    Core,
    Wiring,
    Mechanical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub code: CodeRef,
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<Tag>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Tag {
    Subtle,
    Breaking,
    Race,
    Perf,
}

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
                            format!(" ({})", wire(&tag))
                        });
                        format!(
                            "- {}: {}{tag}\n",
                            change.code.link(),
                            change.note
                        )
                    })
                    .collect();
                format!("**{}**\n\n{}", heading(group.kind), entries.concat())
            })
            .collect::<Vec<String>>()
            .join("\n")
    }
}

fn heading(kind: ChangeKind) -> String {
    let name = wire(&kind);
    let mut letters = name.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(letters).collect()
    })
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
                wire(kind),
                wire(after)
            ),
            Self::Twice { kind } => write!(
                out,
                "the `{}` group comes twice: merge them into one",
                wire(kind)
            ),
        }
    }
}

impl core::error::Error for ChangesError {}
