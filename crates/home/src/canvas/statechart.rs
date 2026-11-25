use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{CanvasError, distinct, quoted, short};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Statechart {
    pub states: Vec<State>,
    pub start: String,
    pub transitions: Vec<Transition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marked: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    pub from: String,
    pub to: String,
    pub event: String,
}

impl Statechart {
    pub(super) fn check(&self) -> Result<(), CanvasError> {
        distinct("state", self.states.iter().map(|state| state.id.as_str()))?;
        short(self.states.iter().map(|state| &state.label).chain(
            self.transitions.iter().map(|transition| &transition.event),
        ))?;
        self.walkable().map_err(CanvasError::Statechart)
    }

    fn walkable(&self) -> Result<(), StatechartError> {
        let known = |id: &str| self.index(id).is_some();
        if !known(&self.start) {
            return Err(StatechartError::UnknownStart {
                start: self.start.clone(),
            });
        }
        for transition in &self.transitions {
            if !known(&transition.from) {
                return Err(StatechartError::UnknownFrom {
                    state: transition.from.clone(),
                });
            }
            if !known(&transition.to) {
                return Err(StatechartError::UnknownTo {
                    state: transition.to.clone(),
                });
            }
        }
        if let Some(first) =
            self.path.first().filter(|first| **first != self.start)
        {
            return Err(StatechartError::PathStart {
                start: self.start.clone(),
                first: first.clone(),
            });
        }
        self.path
            .iter()
            .zip(self.path.iter().skip(1))
            .find(|&(from, to)| {
                !self.transitions.iter().any(|transition| {
                    transition.from == *from && transition.to == *to
                })
            })
            .map_or(Ok(()), |(from, to)| {
                Err(StatechartError::OffPath {
                    from: from.clone(),
                    to: to.clone(),
                })
            })
    }

    fn index(&self, id: &str) -> Option<usize> {
        self.states.iter().position(|state| state.id == id)
    }

    fn named(&self, id: &str) -> String {
        self.index(id)
            .map_or_else(String::new, |at| format!("s{at}"))
    }

    pub(super) fn mermaid(&self) -> String {
        let states = self.states.iter().enumerate().map(|(at, state)| {
            format!("state {} as s{at}", quoted(&state.label))
        });
        let start =
            core::iter::once(format!("[*] --> {}", self.named(&self.start)));
        let transitions = self.transitions.iter().map(|transition| {
            format!(
                "{} --> {} : {}",
                self.named(&transition.from),
                self.named(&transition.to),
                spoken(&transition.event)
            )
        });
        let notes = self.states.iter().enumerate().filter_map(|(at, state)| {
            state.marked.as_ref().map(|reason| {
                format!("note right of s{at} : {}", spoken(reason))
            })
        });
        let marked: Vec<String> = self
            .states
            .iter()
            .enumerate()
            .filter(|(_, state)| state.marked.is_some())
            .map(|(at, _)| format!("s{at}"))
            .collect();
        let mut path: Vec<String> = Vec::new();
        for id in self.path.iter().map(|id| self.named(id)) {
            if !path.contains(&id) {
                path.push(id);
            }
        }
        let styles = styled(&marked, "marked", "fill:#fde68a,stroke:#b45309")
            .into_iter()
            .chain(styled(&path, "path", "stroke:#2563eb,stroke-width:3px"));
        let lines: Vec<String> = states
            .chain(start)
            .chain(transitions)
            .chain(notes)
            .chain(styles)
            .collect();
        format!(
            "```mermaid\nstateDiagram-v2\n    {}\n```\n",
            lines.join("\n    ")
        )
    }
}

fn spoken(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
        .replace(':', "#58;")
}

fn styled(ids: &[String], class: &str, style: &str) -> Vec<String> {
    if ids.is_empty() {
        return Vec::new();
    }
    Vec::from([
        format!("classDef {class} {style}"),
        format!("class {} {class}", ids.join(",")),
    ])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatechartError {
    UnknownStart { start: String },
    UnknownFrom { state: String },
    UnknownTo { state: String },
    PathStart { start: String, first: String },
    OffPath { from: String, to: String },
}

impl fmt::Display for StatechartError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownStart { start } => write!(
                out,
                "the statechart starts at `{start}`, which isn't a state: add the state or change `start`"
            ),
            Self::UnknownFrom { state } => write!(
                out,
                "a transition comes from `{state}`, which isn't a state: add the state or change the transition"
            ),
            Self::UnknownTo { state } => write!(
                out,
                "a transition goes to `{state}`, which isn't a state: add the state or change the transition"
            ),
            Self::OffPath { from, to } => write!(
                out,
                "the path goes from `{from}` to `{to}`, but no transition does: add that transition or change the path"
            ),
            Self::PathStart { start, first } => write!(
                out,
                "the path begins at `{first}`, but the statechart starts at `{start}`: begin the path with `{start}`"
            ),
        }
    }
}

impl core::error::Error for StatechartError {}
