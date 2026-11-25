use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::CanvasError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Progress {
    pub label: String,
    pub bar: Bar,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Bar {
    Count { done: u64, total: u64 },
    Stretches(Vec<Stretch>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Stretch {
    pub label: String,
    pub done: u64,
    pub total: u64,
}

const CELLS: u64 = 10;

impl Progress {
    pub(super) fn check(&self) -> Result<(), CanvasError> {
        let over = |label: &str, done: u64, total: u64| {
            (done > total).then(|| ProgressError::OverTotal {
                label: String::from(label),
                done,
                total,
            })
        };
        let found = match &self.bar {
            Bar::Count { done, total } => over(&self.label, *done, *total),
            Bar::Stretches(stretches) => stretches.iter().find_map(|stretch| {
                over(&stretch.label, stretch.done, stretch.total)
            }),
        };
        found.map_or(Ok(()), |error| Err(CanvasError::Progress(error)))
    }

    pub(super) fn markdown(&self) -> String {
        let label = &self.label;
        match &self.bar {
            Bar::Count { done, total } => {
                format!("**{label}** {}\n", drawn(*done, *total))
            }
            Bar::Stretches(stretches) => {
                let lines: Vec<String> = stretches
                    .iter()
                    .map(|stretch| {
                        format!(
                            "- {} {}\n",
                            stretch.label,
                            drawn(stretch.done, stretch.total)
                        )
                    })
                    .collect();
                format!("**{label}**\n\n{}", lines.concat())
            }
        }
    }
}

fn drawn(done: u64, total: u64) -> String {
    let filled = done.saturating_mul(CELLS).checked_div(total).unwrap_or(0);
    let empty = CELLS.saturating_sub(filled);
    let cells = |count: u64, cell: &str| {
        cell.repeat(usize::try_from(count).unwrap_or_default())
    };
    format!(
        "`{}{}` {done}/{total}",
        cells(filled, "█"),
        cells(empty, "░")
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressError {
    OverTotal {
        label: String,
        done: u64,
        total: u64,
    },
}

impl fmt::Display for ProgressError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OverTotal { label, done, total } => write!(
                out,
                "`{label}` has {done} done of {total}: make `done` at most `total`"
            ),
        }
    }
}

impl core::error::Error for ProgressError {}
