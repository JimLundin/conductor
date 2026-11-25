use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::CanvasError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub question: String,
    pub options: Vec<Choice>,
    pub pick: usize,
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub label: String,
    pub gets: String,
    pub costs: String,
}

impl Options {
    pub(super) fn check(&self) -> Result<(), CanvasError> {
        let pick = self.pick;
        match self.options.len().checked_sub(1) {
            None | Some(0) => Err(OptionsError::TooFew {
                question: self.question.clone(),
            }),
            Some(last) if pick > last => {
                Err(OptionsError::PastLast { pick, last })
            }
            Some(_) => Ok(()),
        }
        .map_err(CanvasError::Options)
    }

    pub(super) fn markdown(&self) -> String {
        let choices: Vec<String> = self
            .options
            .iter()
            .enumerate()
            .map(|(at, choice)| {
                let mark = if at == self.pick {
                    " (recommended)"
                } else {
                    ""
                };
                format!(
                    "- **{}**{mark}: gets {}; costs {}\n",
                    choice.label, choice.gets, choice.costs
                )
            })
            .collect();
        let recommended = self
            .options
            .get(self.pick)
            .map_or("", |choice| choice.label.as_str());
        format!(
            "**{}**\n\n{}\nRecommended **{recommended}**: {}\n",
            self.question,
            choices.concat(),
            self.why
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionsError {
    TooFew { question: String },
    PastLast { pick: usize, last: usize },
}

impl fmt::Display for OptionsError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFew { question } => write!(
                out,
                "`{question}` has fewer than two options: add the choices the pick is made from"
            ),
            Self::PastLast { pick, last } => write!(
                out,
                "`pick` is {pick}, past the last of {} options: pick one from 0 to {last}",
                last.saturating_add(1)
            ),
        }
    }
}

impl core::error::Error for OptionsError {}
