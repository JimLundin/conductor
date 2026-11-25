use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::CodeRef;

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

impl ChangeKind {
    const fn name(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Wiring => "wiring",
            Self::Mechanical => "mechanical",
        }
    }

    const fn heading(self) -> &'static str {
        match self {
            Self::Core => "Core",
            Self::Wiring => "Wiring",
            Self::Mechanical => "Mechanical",
        }
    }
}

impl Tag {
    const fn name(self) -> &'static str {
        match self {
            Self::Subtle => "subtle",
            Self::Breaking => "breaking",
            Self::Race => "race",
            Self::Perf => "perf",
        }
    }
}

pub(super) fn check(groups: &[ChangeGroup]) -> Result<(), ChangesError> {
    groups
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
        .map_or(Ok(()), Err)
}

pub(super) fn markdown(groups: &[ChangeGroup]) -> String {
    groups
        .iter()
        .map(|group| {
            let entries: Vec<String> = group
                .entries
                .iter()
                .map(|change| {
                    let tag = change.tag.map_or_else(String::new, |tag| {
                        format!(" ({})", tag.name())
                    });
                    format!("- {}: {}{tag}\n", change.code.link(), change.note)
                })
                .collect();
            format!("**{}**\n\n{}", group.kind.heading(), entries.concat())
        })
        .collect::<Vec<String>>()
        .join("\n")
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
                kind.name(),
                after.name()
            ),
            Self::Twice { kind } => write!(
                out,
                "the `{}` group comes twice: merge them into one",
                kind.name()
            ),
        }
    }
}

impl core::error::Error for ChangesError {}
