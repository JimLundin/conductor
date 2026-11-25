use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{CanvasError, distinct, quoted, short};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Diagram {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    #[serde(default)]
    pub groups: Vec<Group>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub step: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub step: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub id: String,
    pub label: String,
}

impl Diagram {
    pub(super) fn check(&self) -> Result<(), CanvasError> {
        distinct("node", self.nodes.iter().map(|node| node.id.as_str()))?;
        distinct("group", self.groups.iter().map(|group| group.id.as_str()))?;
        self.wired().map_err(CanvasError::Diagram)?;
        short(
            self.nodes
                .iter()
                .map(|node| &node.label)
                .chain(self.edges.iter().filter_map(|edge| edge.label.as_ref()))
                .chain(self.groups.iter().map(|group| &group.label)),
        )?;
        self.staged()
            .and_then(|()| self.numbered())
            .and_then(|()| self.connected())
            .map_err(CanvasError::Diagram)
    }

    fn numbered(&self) -> Result<(), DiagramError> {
        let steps = self.steps();
        if steps.contains(&0) {
            return Err(DiagramError::StepZero);
        }
        (1..=steps.last().copied().unwrap_or_default())
            .find(|step| !steps.contains(step))
            .map_or(Ok(()), |step| Err(DiagramError::Skipped { step }))
    }

    fn connected(&self) -> Result<(), DiagramError> {
        self.nodes
            .iter()
            .find(|node| {
                !self.edges.iter().any(|edge| {
                    edge.step <= node.step
                        && (edge.from == node.id || edge.to == node.id)
                })
            })
            .map_or(Ok(()), |node| {
                Err(DiagramError::Alone {
                    node: node.id.clone(),
                    step: node.step,
                })
            })
    }

    fn wired(&self) -> Result<(), DiagramError> {
        if let Some(edge) = self
            .edges
            .iter()
            .find(|edge| !self.has(&edge.from) || !self.has(&edge.to))
        {
            let missing = if self.has(&edge.from) {
                &edge.to
            } else {
                &edge.from
            };
            return Err(DiagramError::Dangling {
                from: edge.from.clone(),
                to: edge.to.clone(),
                missing: missing.clone(),
            });
        }
        if let Some(node) = self.nodes.iter().find(|node| {
            node.group.as_ref().is_some_and(|group| {
                !self.groups.iter().any(|known| &known.id == group)
            })
        }) {
            return Err(DiagramError::Ungrouped {
                node: node.id.clone(),
                group: node.group.clone().unwrap_or_default(),
            });
        }
        Ok(())
    }

    fn staged(&self) -> Result<(), DiagramError> {
        if let Some((edge, node)) = self.edges.iter().find_map(|edge| {
            self.nodes
                .iter()
                .filter(|node| node.id == edge.from || node.id == edge.to)
                .find(|node| node.step > edge.step)
                .map(|node| (edge, node))
        }) {
            return Err(DiagramError::Early {
                from: edge.from.clone(),
                to: edge.to.clone(),
                step: edge.step,
                node: node.id.clone(),
                shown: node.step,
            });
        }
        self.steps()
            .into_iter()
            .map(|step| (step, self.shown(step).count()))
            .find(|&(_, nodes)| nodes > MOST_NODES)
            .map_or(Ok(()), |(step, nodes)| {
                Err(DiagramError::Crowded { step, nodes })
            })
    }

    pub(super) fn mermaid(&self) -> String {
        self.steps()
            .into_iter()
            .map(|step| self.flowchart(step))
            .collect::<Vec<String>>()
            .join("\n")
    }

    fn has(&self, id: &str) -> bool {
        self.nodes.iter().any(|node| node.id == id)
    }

    fn steps(&self) -> BTreeSet<u8> {
        self.nodes
            .iter()
            .map(|node| node.step)
            .chain(self.edges.iter().map(|edge| edge.step))
            .collect()
    }

    fn shown(&self, step: u8) -> impl Iterator<Item = (usize, &Node)> {
        self.nodes
            .iter()
            .enumerate()
            .filter(move |(_, node)| node.step <= step)
    }

    fn index(&self, id: &str, step: u8) -> Option<usize> {
        self.shown(step)
            .find(|(_, node)| node.id == id)
            .map(|(at, _)| at)
    }

    fn flowchart(&self, step: u8) -> String {
        let mut lines = Vec::from([String::from("```mermaid\nflowchart LR")]);
        lines.extend(
            self.shown(step)
                .filter(|(_, node)| node.group.is_none())
                .map(|(at, node)| {
                    format!("    n{at}[{}]", quoted(&node.label))
                }),
        );
        for (place, group) in self.groups.iter().enumerate() {
            let inside: Vec<String> = self
                .shown(step)
                .filter(|(_, node)| node.group.as_ref() == Some(&group.id))
                .map(|(at, node)| {
                    format!("        n{at}[{}]", quoted(&node.label))
                })
                .collect();
            if !inside.is_empty() {
                lines.push(format!(
                    "    subgraph g{place}[{}]",
                    quoted(&group.label)
                ));
                lines.extend(inside);
                lines.push(String::from("    end"));
            }
        }
        lines.extend(
            self.edges
                .iter()
                .filter(|edge| edge.step <= step)
                .filter_map(|edge| {
                    let from = self.index(&edge.from, step)?;
                    let to = self.index(&edge.to, step)?;
                    Some(edge.label.as_ref().map_or_else(
                        || format!("    n{from} --> n{to}"),
                        |label| {
                            format!("    n{from} -->|{}| n{to}", quoted(label))
                        },
                    ))
                }),
        );
        lines.push(String::from("```\n"));
        lines.join("\n")
    }
}

const MOST_NODES: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagramError {
    Crowded {
        step: u8,
        nodes: usize,
    },
    Dangling {
        from: String,
        to: String,
        missing: String,
    },
    Ungrouped {
        node: String,
        group: String,
    },
    Early {
        from: String,
        to: String,
        step: u8,
        node: String,
        shown: u8,
    },
    StepZero,
    Skipped {
        step: u8,
    },
    Alone {
        node: String,
        step: u8,
    },
}

impl fmt::Display for DiagramError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Crowded { step, nodes } => write!(
                out,
                "step {step} of a diagram shows {nodes} nodes, over {MOST_NODES}: split it into steps or diagrams"
            ),
            Self::Dangling { from, to, missing } => write!(
                out,
                "an edge runs from `{from}` to `{to}`, but the diagram has no node `{missing}`: add the node or drop the edge"
            ),
            Self::Ungrouped { node, group } => write!(
                out,
                "node `{node}` is in group `{group}`, but the diagram has no group `{group}`: add the group or take the node out of it"
            ),
            Self::Early {
                from,
                to,
                step,
                node,
                shown,
            } => write!(
                out,
                "the edge from `{from}` to `{to}` comes at step {step}, before node `{node}` at step {shown}: move the edge to step {shown} or later"
            ),
            Self::StepZero => write!(
                out,
                "a diagram has a step 0: number its steps from 1 with no gap"
            ),
            Self::Skipped { step } => write!(
                out,
                "a diagram skips step {step}: number its steps from 1 with no gap"
            ),
            Self::Alone { node, step } => write!(
                out,
                "node `{node}` stands alone at step {step}: bring the node in with the edge that connects it"
            ),
        }
    }
}
