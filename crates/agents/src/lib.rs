//! The provider-agnostic agent API a session speaks: the runtime protocol
//! of commands and events, the steps a provider's events map to and the
//! output it produces, the replies it answers requests with, how its
//! process starts and how it ended, and the profile of its CLI the host
//! starts and signs it in by. It owns the agent vocabulary: tool calls,
//! questions and answers, checklists, sub-agents, outcomes, models and
//! effort, credential forms and the providers. Each provider's wire maps
//! into it (`claude`, `codex`).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod checklist;
mod credential;
pub mod driver;
mod exit;
mod model;
mod note;
mod outcome;
mod output;
pub mod pin;
mod prelude;
mod profile;
mod provider;
mod question;
mod run;
pub mod runtime;
mod sign_in;
mod step;
mod subagent;
mod tool;

pub use checklist::{ChecklistEntry, ChecklistStatus};
pub use credential::{
    Account, Auth, Credential, CredentialForm, Field, FieldKind, ProviderInfo,
};
pub use exit::Exit;
pub use model::{Effort, ModelChoice, ModelId, Models};
pub use note::Note;
pub use outcome::{CloseReason, TurnOutcome};
pub use output::Output;
pub use profile::{Config, Profile, Reported};
pub use provider::Provider;
pub use question::{
    Answer, ClientName, Decision, Question, QuestionOption, Selection,
};
pub use run::{HomeKind, Invocation, Var};
pub use sign_in::{Failure, Pick, Shown, SignIn, SignInCode, Word, plain};
pub use step::{
    NativeRequest, NativeTurn, Reply, Step, reasoning, reasoning_delta,
};
pub use subagent::{SubagentActivity, SubagentId, SubagentOutcome};
pub use tool::{
    Change, FileChange, ToolCallId, ToolKind, ToolOutcome, ToolPhase,
};

/// The credential took. Discovery may still fail, in which case the host
/// decides whether the credential's named model can be offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configuration {
    pub named: Option<ModelId>,
    pub models: Result<Models, alloc::string::String>,
}
