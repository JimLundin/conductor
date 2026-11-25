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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configuration {
    pub named: Option<ModelId>,
    pub models: Result<Models, alloc::string::String>,
}
