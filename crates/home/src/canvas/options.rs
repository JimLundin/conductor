use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub label: String,
    pub gets: String,
    pub costs: String,
}

pub(super) fn check(
    question: &str,
    options: &[Choice],
    pick: usize,
) -> Result<(), OptionsError> {
    match options.len().checked_sub(1) {
        None => Err(OptionsError::NoOptions {
            question: String::from(question),
        }),
        Some(last) if pick > last => Err(OptionsError::PastLast { pick, last }),
        Some(_) => Ok(()),
    }
}

pub(super) fn markdown(
    question: &str,
    options: &[Choice],
    pick: usize,
    why: &str,
) -> String {
    let choices: Vec<String> = options
        .iter()
        .enumerate()
        .map(|(at, choice)| {
            let mark = if at == pick { " (picked)" } else { "" };
            format!(
                "- **{}**{mark}: gets {}; costs {}\n",
                choice.label, choice.gets, choice.costs
            )
        })
        .collect();
    let picked = options.get(pick).map_or("", |choice| choice.label.as_str());
    format!(
        "**{question}**\n\n{}\nPicked **{picked}**: {why}\n",
        choices.concat()
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionsError {
    NoOptions { question: String },
    PastLast { pick: usize, last: usize },
}

impl fmt::Display for OptionsError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoOptions { question } => write!(
                out,
                "`{question}` has no options: add the choices the pick is made from"
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
