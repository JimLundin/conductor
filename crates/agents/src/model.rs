//! The models a provider's CLI offers and how hard they reason.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::prelude::*;

/// The models a provider's CLI offers on this host, as the CLI lists them
/// when the host starts, and the one a session gets when it names none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Models {
    pub choices: Vec<ModelChoice>,
    pub default: Option<ModelId>,
}

impl Models {
    /// The label of the model picker's first entry, which sends no model.
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

    /// The efforts `model` takes (`None`: the default model), lowest first.
    #[must_use]
    pub fn efforts(&self, model: Option<&ModelId>) -> Vec<Effort> {
        model
            .or(self.default.as_ref())
            .and_then(|id| self.find(id))
            .map_or(Vec::new(), |choice| choice.efforts.clone())
    }

    /// The effort to keep when the model changes to `model`: `effort` if the
    /// model takes it, else the model's default.
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
    /// The efforts this model takes, lowest first.
    pub efforts: Vec<Effort>,
}

/// A model by the name its CLI takes (`haiku`, `gpt-6-luna`).
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct ModelId(pub String);

/// How hard the model reasons: Claude's `--effort`, Codex's
/// `model_reasoning_effort`. Both CLIs spell the levels the same way.
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

impl Effort {
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

    /// A level a CLI lists. Any other, such as Codex's `minimal`, is `None`.
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
}

#[cfg(test)]
mod tests {
    use super::{Effort, ModelChoice, ModelId, Models};

    #[test]
    fn the_pickers_offer_the_default_then_the_models_and_their_efforts() {
        let models = Models {
            choices: Vec::from([
                ModelChoice {
                    id: ModelId(String::from("opus")),
                    name: String::from("Opus"),
                    description: String::from("best"),
                    efforts: Vec::from([Effort::Low, Effort::Max]),
                },
                ModelChoice {
                    id: ModelId(String::from("haiku")),
                    name: String::from("Haiku"),
                    description: String::from("fast"),
                    efforts: Vec::from([Effort::High]),
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
                Vec::from([Effort::Low, Effort::Max]),
                Vec::from([Effort::High]),
                Vec::new(),
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
    fn a_model_is_found_by_its_id() {
        let choice = |id: &str| ModelChoice {
            id: ModelId(String::from(id)),
            name: String::from(id),
            description: String::new(),
            efforts: Vec::new(),
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
