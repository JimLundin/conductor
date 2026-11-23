//! What an agent asks the user, and how the user answers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::prelude::*;

/// One question. Both agents take any typed text as an answer, so every
/// question also allows one besides its options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Question {
    /// A short label for the question, such as "Color".
    pub header: String,
    pub text: String,
    pub options: Vec<QuestionOption>,
    pub selection: Selection,
}

/// How many options the user may choose.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum Selection {
    One,
    Many,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

/// The answer to one question: the labels of the options chosen, and any
/// text the user typed instead, in that order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Answer(pub Vec<String>);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum Decision {
    Approve,
    /// Approves this call and the calls the request's `for_session` names,
    /// until the session ends.
    ApproveForSession,
    Deny,
}

/// The name a client gave when it connected: who answered a request, or
/// sent a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClientName(pub String);
