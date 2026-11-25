use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::wordy;

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
    pub(super) fn check(&self) -> Result<(), StatechartError> {
        let known = |id: &str| self.states.iter().any(|state| state.id == id);
        self.states
            .iter()
            .map(|state| &state.label)
            .chain(self.transitions.iter().map(|transition| &transition.event))
            .find(|label| wordy(label))
            .map_or(Ok(()), |label| {
                Err(StatechartError::LongLabel {
                    label: label.clone(),
                })
            })?;
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
        self.walked()
            .zip(self.path.iter())
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

    fn walked(&self) -> impl Iterator<Item = &String> {
        core::iter::once(&self.start).chain(self.path.iter())
    }

    pub(super) fn mermaid(&self) -> String {
        let states = self
            .states
            .iter()
            .map(|state| format!("state \"{}\" as {}", state.label, state.id));
        let start = core::iter::once(format!("[*] --> {}", self.start));
        let transitions = self.transitions.iter().map(|transition| {
            format!(
                "{} --> {} : {}",
                transition.from, transition.to, transition.event
            )
        });
        let notes = self.states.iter().filter_map(|state| {
            state
                .marked
                .as_ref()
                .map(|reason| format!("note right of {} : {reason}", state.id))
        });
        let marked: Vec<&str> = self
            .states
            .iter()
            .filter(|state| state.marked.is_some())
            .map(|state| state.id.as_str())
            .collect();
        let mut path: Vec<&str> = Vec::new();
        if !self.path.is_empty() {
            for id in self.walked() {
                if !path.contains(&id.as_str()) {
                    path.push(id);
                }
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

fn styled(ids: &[&str], class: &str, style: &str) -> Vec<String> {
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
    LongLabel { label: String },
    UnknownStart { start: String },
    UnknownFrom { state: String },
    UnknownTo { state: String },
    OffPath { from: String, to: String },
}

impl fmt::Display for StatechartError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LongLabel { label } => write!(
                out,
                "the label `{label}` is over eight words: put the detail in prose"
            ),
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
        }
    }
}

impl core::error::Error for StatechartError {}
