use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use alloc::collections::BTreeSet;
use core::cmp::Ordering;

use crate::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Models {
    pub choices: Vec<ModelChoice>,
    pub default: Option<ModelId>,
}

impl Models {
    #[must_use]
    pub fn default_label(&self) -> String {
        self.default.as_ref().map_or_else(
            || String::from("Default"),
            |id| {
                format!(
                    "Default ({})",
                    self.find(id)
                        .map_or(id.0.as_str(), |choice| choice.name.as_str())
                )
            },
        )
    }

    #[must_use]
    pub fn efforts(&self, model: Option<&ModelId>) -> BTreeSet<Effort> {
        model
            .or(self.default.as_ref())
            .and_then(|id| self.find(id))
            .map_or_else(BTreeSet::new, |choice| choice.efforts.clone())
    }

    #[must_use]
    pub fn kept_effort(
        &self,
        model: Option<&ModelId>,
        effort: Option<Effort>,
    ) -> Option<Effort> {
        effort.filter(|kept| self.efforts(model).contains(kept))
    }

    #[must_use]
    pub const fn none() -> Self {
        Self {
            choices: Vec::new(),
            default: None,
        }
    }

    #[must_use]
    pub fn find(&self, id: &ModelId) -> Option<&ModelChoice> {
        self.choices.iter().find(|choice| &choice.id == id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelChoice {
    pub id: ModelId,
    pub name: String,
    pub description: String,
    pub efforts: BTreeSet<Effort>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct ModelId(pub String);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum Effort {
    Low,
    Medium,
    High,
    XHigh,
    Max,
    Ultra,
}

impl Ord for Effort {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank().cmp(&other.rank())
    }
}

impl PartialOrd for Effort {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Effort {
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Low => 0,
            Self::Medium => 1,
            Self::High => 2,
            Self::XHigh => 3,
            Self::Max => 4,
            Self::Ultra => 5,
        }
    }

    #[must_use]
    pub const fn wire(&self) -> &str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
            Self::Ultra => "ultra",
        }
    }

    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::XHigh),
            "max" => Some(Self::Max),
            "ultra" => Some(Self::Ultra),
            _ => None,
        }
    }

    #[must_use]
    pub fn listed(words: &[&str]) -> BTreeSet<Self> {
        words.iter().copied().filter_map(Self::parse).collect()
    }
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;

    use super::{Effort, ModelChoice, ModelId, Models};

    #[test]
    fn the_pickers_offer_the_default_then_the_models_and_their_efforts() {
        let models = Models {
            choices: Vec::from([
                ModelChoice {
                    id: ModelId(String::from("opus")),
                    name: String::from("Opus"),
                    description: String::from("best"),
                    efforts: BTreeSet::from([Effort::Low, Effort::Max]),
                },
                ModelChoice {
                    id: ModelId(String::from("haiku")),
                    name: String::from("Haiku"),
                    description: String::from("fast"),
                    efforts: BTreeSet::from([Effort::High]),
                },
            ]),
            default: Some(ModelId(String::from("opus"))),
        };
        let haiku = ModelId(String::from("haiku"));
        assert_eq!(
            (
                models.default_label(),
                Models::none().default_label(),
                models.efforts(None),
                models.efforts(Some(&haiku)),
                Models::none().efforts(None),
                models.kept_effort(Some(&haiku), Some(Effort::Max)),
                models.kept_effort(Some(&haiku), Some(Effort::High)),
            ),
            (
                String::from("Default (Opus)"),
                String::from("Default"),
                BTreeSet::from([Effort::Low, Effort::Max]),
                BTreeSet::from([Effort::High]),
                BTreeSet::new(),
                None,
                Some(Effort::High),
            )
        );
    }

    #[test]
    fn each_effort_reads_back_from_its_word() {
        let all = [
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::XHigh,
            Effort::Max,
            Effort::Ultra,
        ];
        assert_eq!(
            all.iter().map(Effort::wire).collect::<Vec<_>>(),
            ["low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(
            all.map(|effort| Effort::parse(effort.wire())),
            all.map(Some)
        );
        assert_eq!(Effort::parse("minimal"), None);
    }

    #[test]
    fn a_listing_gives_its_known_efforts_lowest_first_once_each() {
        assert_eq!(
            Effort::listed(&["max", "minimal", "low", "high", "low"])
                .into_iter()
                .collect::<Vec<_>>(),
            [Effort::Low, Effort::High, Effort::Max]
        );
    }

    #[test]
    fn efforts_order_by_rank_from_low_to_ultra() {
        let ranked = BTreeSet::from([
            Effort::Ultra,
            Effort::Max,
            Effort::XHigh,
            Effort::High,
            Effort::Medium,
            Effort::Low,
        ]);
        assert_eq!(
            ranked.iter().map(Effort::wire).collect::<Vec<_>>(),
            ["low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(
            ranked.into_iter().map(Effort::rank).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4, 5]
        );
    }

    #[test]
    fn a_model_is_found_by_its_id() {
        let choice = |id: &str| ModelChoice {
            id: ModelId(String::from(id)),
            name: String::from(id),
            description: String::new(),
            efforts: BTreeSet::new(),
        };
        let models = Models {
            choices: Vec::from([choice("opus"), choice("haiku")]),
            default: None,
        };
        assert_eq!(
            models.find(&ModelId(String::from("haiku"))),
            Some(&choice("haiku"))
        );
        assert_eq!(models.find(&ModelId(String::from("sonnet"))), None);
    }
}
