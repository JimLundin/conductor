use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{CanvasError, distinct, line, quoted, short};

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
        self.walked().map(drop).map_err(CanvasError::Statechart)
    }

    fn walked(&self) -> Result<Walk<'_>, StatechartError> {
        let start = self.index(&self.start).ok_or_else(|| {
            StatechartError::UnknownStart {
                start: self.start.clone(),
            }
        })?;
        let steps = self
            .transitions
            .iter()
            .map(|transition| {
                let from = self.index(&transition.from).ok_or_else(|| {
                    StatechartError::UnknownFrom {
                        state: transition.from.clone(),
                    }
                })?;
                let to = self.index(&transition.to).ok_or_else(|| {
                    StatechartError::UnknownTo {
                        state: transition.to.clone(),
                    }
                })?;
                Ok((from, to, transition.event.as_str()))
            })
            .collect::<Result<Vec<Step<'_>>, StatechartError>>()?;
        let mut path = Vec::new();
        if let Some(first) = self.path.first() {
            if *first != self.start {
                return Err(StatechartError::PathStart {
                    start: self.start.clone(),
                    first: first.clone(),
                });
            }
            path.push(start);
        }
        for (from, to) in self.path.iter().zip(self.path.iter().skip(1)) {
            let (_, &(_, at, _)) = self
                .transitions
                .iter()
                .zip(&steps)
                .find(|(transition, _)| {
                    transition.from == *from && transition.to == *to
                })
                .ok_or_else(|| StatechartError::OffPath {
                    from: from.clone(),
                    to: to.clone(),
                })?;
            if !path.contains(&at) {
                path.push(at);
            }
        }
        Ok(Walk { start, steps, path })
    }

    fn index(&self, id: &str) -> Option<usize> {
        self.states.iter().position(|state| state.id == id)
    }

    pub(super) fn mermaid(&self) -> Result<String, StatechartError> {
        let walk = self.walked()?;
        let states = self.states.iter().enumerate().map(|(at, state)| {
            format!("state {} as s{at}", quoted(&state.label))
        });
        let start = core::iter::once(format!("[*] --> s{}", walk.start));
        let transitions = walk.steps.iter().map(|&(from, to, event)| {
            format!("s{from} --> s{to} : {}", spoken(event))
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
        let path: Vec<String> =
            walk.path.iter().map(|at| format!("s{at}")).collect();
        let styles = styled(&marked, "marked", "fill:#fde68a,stroke:#b45309")
            .into_iter()
            .chain(styled(&path, "path", "stroke:#2563eb,stroke-width:3px"));
        let lines: Vec<String> = states
            .chain(start)
            .chain(transitions)
            .chain(notes)
            .chain(styles)
            .collect();
        Ok(format!(
            "```mermaid\nstateDiagram-v2\n    {}\n```\n",
            lines.join("\n    ")
        ))
    }
}

type Step<'a> = (usize, usize, &'a str);

struct Walk<'a> {
    start: usize,
    steps: Vec<Step<'a>>,
    path: Vec<usize>,
}

fn spoken(text: &str) -> String {
    line(text).replace(':', "#58;")
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
